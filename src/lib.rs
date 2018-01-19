

extern crate futures;
extern crate tokio_core;
extern crate thread_id;
extern crate parking_lot;

#[cfg(test)]
extern crate tokio_timer;

#[macro_use]
extern crate log;
extern crate pretty_env_logger;

/// Error handling.
mod error;
pub use error::*;

mod utils;

use std::fmt;

use futures::{
  Future,
  lazy
};
use futures::sync::oneshot::{
  Receiver as OneshotReceiver,
  channel as oneshot_channel
};

use tokio_core::reactor::{
  Core,
  Handle
};

use std::sync::Arc;
use parking_lot::RwLock;

use std::ops::{
  Deref,
  DerefMut
};

use std::thread::Builder;

use utils::{
  CancelSender,
  ExitSender,
};

#[doc(hidden)]
pub type CancelHookFt<T: Send + 'static> = Box<Future<Item=T, Error=()>>;

/// Options when `spawn`ing an event loop thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadOptions {
  pub name: String,
  pub stack_size: Option<usize>
}

pub struct Hooks<T: Send + 'static>  {
  /// A function that runs when the event loop is canceled via the `cancel` function, but before it is interrupted.
  on_cancel: Arc<RwLock<Option<CancelHookFt<T>>>>

  // TODO on_exit
}

impl<T: Send + 'static> Clone for Hooks<T> {
  fn clone(&self) -> Self {
    Hooks { on_cancel: self.on_cancel.clone() }
  }
}

impl<T: Send + 'static> Hooks<T> {

  pub fn new() -> Hooks<T> {
    Hooks {
      on_cancel: Arc::new(RwLock::new(None))
    }
  }

  /// Register a function to be called on the event loop when `cancel` is called.
  pub fn on_cancel<F: FnOnce() -> Box<Future<Item=T, Error=()>> + 'static>(&self, func: F) {
    let mut cancel_guard = self.on_cancel.write();
    let cancel_ref = cancel_guard.deref_mut();

    let ft = Box::new(lazy(func));
    *cancel_ref = Some(ft);
  }

  #[doc(hidden)]
  pub fn cancel_ref(&self) -> &Arc<RwLock<Option<CancelHookFt<T>>>> {
    &self.on_cancel
  }

}

/// A module for starting event loop threads with notifications when they exit, and with hooks for canceling any running futures.
///
/// Any cloned instances will refer to the same child thread and event loop.
#[derive(Clone)]
pub struct Wombo<T: Send + 'static> {
  core_thread_id: Arc<RwLock<Option<usize>>>,
  cancel_tx: Arc<RwLock<Option<CancelSender>>>,
  exit_tx: Arc<RwLock<Option<ExitSender<T>>>>
}

impl<T: Send + 'static> fmt::Debug for Wombo<T> {

  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "[Wombo]")
  }

}

impl<T: Send + 'static> Wombo<T> {

  /// Create a new `Wombo` instance without initializing the event loop thread.
  pub fn new() -> Wombo<T> {
    Wombo {
      core_thread_id: Arc::new(RwLock::new(None)),
      cancel_tx: Arc::new(RwLock::new(None)),
      exit_tx: Arc::new(RwLock::new(None))
    }
  }

  /// Spawn a child thread running an event loop, running `func` on the child thread before the `core` is `run`.
  ///
  /// To register a callback that should run when `cancel` is called, but before the event loop is interrupted,
  /// use `on_cancel` here.
  ///
  /// # Panics
  ///
  /// Panics if `Core::new()` returns an error creating the event loop on the child thread.
  pub fn spawn<F>(&self, mut options: ThreadOptions, func: F) -> Result<(), WomboError>
    where F: FnOnce(&Handle, Hooks<T>) -> Box<Future<Item=T, Error=()>> + Send + 'static
  {
    let _ = utils::check_not_initialized(&self.core_thread_id)?;

    let stack_size = options.stack_size.take();
    let mut builder = Builder::new().name(options.name);

    if let Some(size) = stack_size {
      builder = builder.stack_size(size);
    }

    let (_core_thread_id, _cancel_tx, _exit_tx) = (
      self.core_thread_id.clone(),
      self.cancel_tx.clone(),
      self.exit_tx.clone()
    );

    let _ = builder.spawn(move || {
      utils::set_thread_id(&_core_thread_id);

      let mut core = match Core::new() {
        Ok(c) => c,
        Err(e) => panic!("Error creating event loop: {:?}", e)
      };
      let handle = core.handle();

      let hooks = Hooks::new();
      let user_ft = func(&handle, hooks.clone());
      let on_cancel = hooks.cancel_ref().clone();

      let (interrupt_tx, user_ft) = utils::interruptible_future(user_ft, on_cancel);
      utils::set_cancel_tx(&_cancel_tx, interrupt_tx);

      let res = match core.run(user_ft) {
        Ok(t) => t,
        Err(e) => {
          error!("Error with event loop thread: {:?}", e);
          None
        }
      };
      utils::clear_thread_id(&_core_thread_id);

      if let Some(exit_tx) = utils::take_exit_tx(&_exit_tx) {
        let _ = exit_tx.send(res);
      }
      let _ = utils::take_cancel_tx(&_cancel_tx);
    })?;

    Ok(())
  }

  /// Whether or not the event loop thread is running.
  pub fn is_running(&self) -> bool {
    let id_guard = self.core_thread_id.read();
    id_guard.deref().is_some()
  }

  /// Send a message to cancel the event loop thread, interrupting any futures running there.
  ///
  /// This first triggers the callback from `on_cancel`, if defined, then interrupts the event loop
  /// and causes the spawned thread to exit. A message is then emitted to the receiver returned by
  /// `on_exit`, and resets the state of this instance so `spawn` can be called again.
  pub fn cancel(&self) -> Result<(), WomboError> {
    let _ = utils::check_initialized(&self.core_thread_id)?;

    let tx = match utils::take_cancel_tx(&self.cancel_tx) {
      Some(tx) => tx,
      None => return Err(WomboError::new(
        WomboErrorKind::NotInitialized, "Cancel sender not initialized."
      ))
    };

    match tx.unbounded_send(true){
      Ok(_) => Ok(()),
      Err(_) => Err(WomboError::new(
        WomboErrorKind::Unknown, "Error sending cancel command."
      ))
    }
  }

  /// Returns a oneshot receiver that resolves when the thread exits for any reason, either via the `cancel` command, or otherwise.
  ///
  /// Upon calling cancel if the caller has registered an `on_cancel` callback then the receiver returned here will resolve with `Some` data,
  /// otherwise it resolves with `None` if no `on_cancel` callback exists.
  ///
  /// If the future returned by `func` in `spawn` resolves before `cancel` is called that data will be sent to the receiver returned here.
  ///
  /// If this is called more than once the previous oneshot channel will be canceled.
  pub fn on_exit(&self) -> Result<OneshotReceiver<Option<T>>, WomboError> {
    let _ = utils::check_initialized(&self.core_thread_id)?;

    let (tx, rx) = oneshot_channel();
    utils::set_exit_tx(&self.exit_tx, tx);
    Ok(rx)
  }

  // TODO add spawn_catch, which enforces UnwindSafe and uses catch_unwind

}

#[cfg(test)]
mod tests {
  #![allow(unused_imports)]

  use tokio_core::reactor::{
    Core,
    Handle
  };

  use super::*;
  use tokio_timer::Timer;
  use std::time::Duration;










}