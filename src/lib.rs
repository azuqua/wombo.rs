//! # Wombo
//!
//! Utilities for managing event loop threads.
//!
//! ## Usage
//!
//! ```rust
//!
//! extern crate futures;
//! extern crate wombo;
//! extern crate tokio_timer;
//!
//! use futures::Future;
//! use tokio_timer::Timer;
//!
//! use std::thread;
//! use std::time::Duration;
//!
//! use wombo::*;
//!
//! const SHOULD_INTERRUPT: bool = true;
//!
//! fn main() {
//!   // expected value when not interrupted
//!   let foo = 1;
//!   // expected value when interrupted
//!   let bar = 2;
//!
//!   let timer = Timer::default();
//!   let sleep_dur = Duration::from_millis(50);
//!
//!   let wombo = Wombo::new();
//!   let options = ThreadOptions::new("loop-1", None);
//!
//!   let spawn_result = wombo.spawn(options, move |handle, hooks| {
//!     // sleep for a second, then return foo, or if canceled return bar
//!     let dur = Duration::from_millis(1000);
//!
//!     hooks.on_cancel(move || Ok(bar));
//!
//!     timer.sleep(dur)
//!       .map_err(|_| ())
//!       .and_then(move |_| Ok(foo))
//!   });
//!
//!   if let Err(e) = spawn_result {
//!     panic!("Error spawning event loop thread: {:?}", e);
//!   }
//!
//!   // give the child thread a chance to initialize
//!   thread::sleep(sleep_dur);
//!
//!   assert!(wombo.is_running());
//!   println!("Wombo {} running thread {:?}", wombo.id(), wombo.core_thread_id());
//!
//!   if SHOULD_INTERRUPT {
//!     if let Err(e) = wombo.cancel() {
//!       panic!("Error canceling: {:?}", e);
//!     }
//!   }
//!
//!   let result = match wombo.on_exit() {
//!     Ok(rx) => rx.wait().unwrap(),
//!     Err(e) => panic!("Error calling on_exit: {:?}", e)
//!   };
//!
//!   if SHOULD_INTERRUPT {
//!     assert_eq!(result, Some(bar));
//!   }else{
//!     assert_eq!(result, Some(foo));
//!   }
//! }
//!
//! ```
//!
//! See [examples](https://github.com/azuqua/wombo.rs/blob/master/examples/README.md) for more.
//!

extern crate futures;
extern crate tokio_core;
extern crate thread_id;
extern crate parking_lot;
extern crate uuid;

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

use std::hash::{
  Hash,
  Hasher
};

use futures::{
  Future,
  lazy,
  IntoFuture
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

/// Options when spawning an event loop thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadOptions {
  /// The name of the child thread.
  pub name: String,
  /// The stack size, in bytes, for the child thread.
  pub stack_size: Option<usize>
}

impl ThreadOptions {

  pub fn new<S: Into<String>>(name: S, stack_size: Option<usize>) -> ThreadOptions {
    ThreadOptions {
      name: name.into(),
      stack_size
    }
  }

}


/// A struct for receiving notifications when the event loop is canceled.
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
  pub fn on_cancel<Fut, F>(&self, func: F)
    where Fut: IntoFuture<Item=T, Error=()> + 'static,
          F: FnOnce() -> Fut + 'static
  {
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
  id: Arc<String>,
  core_thread_id: Arc<RwLock<Option<usize>>>,
  cancel_tx: Arc<RwLock<Option<CancelSender>>>,
  exit_tx: Arc<RwLock<Option<ExitSender<T>>>>
}

impl<T: Send + 'static> fmt::Debug for Wombo<T> {

  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "[Wombo]")
  }

}

impl<T: Send + 'static> PartialEq for Wombo<T> {

  fn eq(&self, other: &Wombo<T>) -> bool {
    self.id() == other.id()
  }

}

impl<T: Send + 'static> Eq for Wombo<T> {}

impl<T: Send + 'static> Hash for Wombo<T> {

  fn hash<H: Hasher>(&self, state: &mut H) {
    self.id.hash(state);
  }

}

impl<T: Send + 'static> Wombo<T> {

  /// Create a new `Wombo` instance without initializing the event loop thread.
  pub fn new() -> Wombo<T> {
    Wombo {
      id: Arc::new(utils::uuid_v4()),
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
  pub fn spawn<Fut, F>(&self, mut options: ThreadOptions, func: F) -> Result<(), WomboError>
    where Fut: IntoFuture<Item=T, Error=()> + 'static,
          F: FnOnce(&Handle, Hooks<T>) -> Fut + Send + 'static
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
      let on_cancel = hooks.cancel_ref().clone();

      let user_ft = Box::new(func(&handle, hooks.clone()).into_future());

      let (interrupt_tx, user_ft) = utils::interruptible_future(user_ft, on_cancel);
      utils::set_cancel_tx(&_cancel_tx, interrupt_tx);

      let res = match core.run(user_ft) {
        Ok(t) => t,
        Err(e) => {
          error!("Error with event loop thread: {:?}", e);
          None
        }
      };
      trace!("Event loop thread exiting...");

      utils::clear_thread_id(&_core_thread_id);

      if let Some(exit_tx) = utils::take_exit_tx(&_exit_tx) {
        let _ = exit_tx.send(res);
      }
      let _ = utils::take_cancel_tx(&_cancel_tx);
    })?;

    Ok(())
  }

  /// Read the ID for this `Wombo` instance. This will not change if the backing thread exits or restarts.
  pub fn id(&self) -> &Arc<String> {
    &self.id
  }

  /// Read the thread ID of the event loop thread.
  pub fn core_thread_id(&self) -> Option<usize> {
    utils::get_thread_id(&self.core_thread_id)
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
  #![allow(dead_code)]
  #![allow(unused_variables)]

  use tokio_core::reactor::{
    Core,
    Handle
  };

  use super::*;
  use tokio_timer::Timer;
  use std::time::Duration;
  use std::thread;

  fn fake_timer_ft(timer: &Timer, dur: u64, c: usize) -> Box<Future<Item=usize, Error=()>> {
    let dur = Duration::from_millis(dur);

    Box::new(timer.sleep(dur).map_err(|_| ()).and_then(move |_| Ok(c)))
  }

  fn sleep_ms(dur: u64) {
    thread::sleep(Duration::from_millis(dur));
  }

  #[test]
  fn should_create_empty_wombo() {
    let _ = pretty_env_logger::init();

    let wombo = Wombo::<usize>::new();
    assert!(!wombo.is_running());
  }

  #[test]
  fn should_read_wombo_metadata() {
    let wombo = Wombo::<usize>::new();

    assert!(wombo.id().len() > 0);
    assert!(wombo.core_thread_id().is_none());
    assert!(!wombo.is_running());
  }

  #[test]
  fn should_error_uninitialized_cancel() {
    let wombo = Wombo::<usize>::new();
    let res = wombo.cancel();

    assert!(res.is_err());
  }

  #[test]
  fn should_error_uninitialized_exit() {
    let wombo = Wombo::<usize>::new();
    let res = wombo.on_exit();

    assert!(res.is_err());
  }

  #[test]
  fn should_spawn_simple_timer_event_loop() {
    let wombo = Wombo::<usize>::new();
    let timer = Timer::default();
    let dur = Duration::from_millis(1000);

    let options = ThreadOptions::new("wombo-loop-1", None);
    let spawn_result = wombo.spawn(options, move |handle, hooks| {
      Box::new(timer.sleep(dur)
        .map_err(|_| ())
        .and_then(|_| Ok(1)))
    });

    if let Err(e) = spawn_result {
      panic!("Error spawning thread: {:?}", e);
    }

    sleep_ms(50);
    assert!(wombo.is_running());
    assert!(wombo.core_thread_id().is_some());

    let rx = match wombo.on_exit() {
      Ok(rx) => rx,
      Err(e) => panic!("Error calling on_exit: {:?}", e)
    };

    let res = rx.wait().unwrap();
    assert_eq!(res, Some(1));
  }

  #[test]
  fn should_spawn_timer_event_loop_with_cancel() {
    let wombo = Wombo::<usize>::new();
    let timer = Timer::default();
    let dur = Duration::from_millis(1000);

    let options = ThreadOptions::new("wombo-loop-2", None);
    let spawn_result = wombo.spawn(options, move |handle, hooks| {
      hooks.on_cancel(|| Ok(2));

      Box::new(timer.sleep(dur)
        .map_err(|_| ())
        .and_then(|_| Ok(1)))
    });

    if let Err(e) = spawn_result {
      panic!("Error spawning thread: {:?}", e);
    }

    sleep_ms(50);
    assert!(wombo.is_running());
    assert!(wombo.core_thread_id().is_some());

    let rx = match wombo.on_exit() {
      Ok(rx) => rx,
      Err(e) => panic!("Error calling on_exit: {:?}", e)
    };

    if let Err(e) = wombo.cancel() {
      panic!("Error canceling wombo: {:?}", e);
    }

    let res = rx.wait().unwrap();
    assert_eq!(res, Some(2));
  }

  #[test]
  fn should_spawn_timer_event_loop_without_cancel() {
    let wombo = Wombo::<usize>::new();
    let timer = Timer::default();
    let dur = Duration::from_millis(1000);

    let options = ThreadOptions::new("wombo-loop-3", None);
    let spawn_result = wombo.spawn(options, move |handle, hooks| {
      hooks.on_cancel(|| Ok(2));

      Box::new(timer.sleep(dur)
        .map_err(|_| ())
        .and_then(|_| Ok(1)))
    });

    if let Err(e) = spawn_result {
      panic!("Error spawning thread: {:?}", e);
    }

    sleep_ms(50);
    assert!(wombo.is_running());
    assert!(wombo.core_thread_id().is_some());

    let rx = match wombo.on_exit() {
      Ok(rx) => rx,
      Err(e) => panic!("Error calling on_exit: {:?}", e)
    };

    let res = rx.wait().unwrap();
    assert_eq!(res, Some(1));
  }

}