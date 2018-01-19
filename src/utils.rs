
use error::*;

use futures::{
  Future,
  Stream,
  future
};
use futures::sync::mpsc::{
  UnboundedSender,
  unbounded as unbounded_channel
};
use futures::sync::oneshot::{
  Sender as OneshotSender
};

use std::sync::Arc;
use parking_lot::RwLock;

use std::ops::{
  Deref,
  DerefMut
};
use super::CancelHookFt;

use thread_id;

use uuid::Uuid;

/// Sends a cancel message to the event loop thread.
pub type CancelSender = UnboundedSender<bool>;
/// Sends an exit message from the event loop thread.
pub type ExitSender<T: Send + 'static> = OneshotSender<Option<T>>;
/// Sender used to interrupt a future.
pub type InterruptSender = UnboundedSender<bool>;

pub fn future_error<T: 'static, E: 'static>(err: E) -> Box<Future<Item=T, Error=E>> {
  Box::new(future::err(err))
}

pub fn future_ok<T: 'static, E: 'static>(d: T) -> Box<Future<Item=T, Error=E>> {
  Box::new(future::ok(d))
}

pub fn uuid_v4() -> String {
  Uuid::new_v4().hyphenated().to_string()
}

/// Take a future and return a new future and a sender that can interrupt the future with a Canceled error.
pub fn interruptible_future<T: Send + 'static>(ft: Box<Future<Item=T, Error=()>>, on_cancel: Arc<RwLock<Option<CancelHookFt<T>>>>)
  -> (InterruptSender, Box<Future<Item=Option<T>, Error=()>>)
{
  let (tx, rx) = unbounded_channel();
  let out_tx = tx.clone();

  let ft = Box::new(ft.then(move |result| {
    let _ = tx.unbounded_send(false);

    match result {
      Ok(t) => Ok(Some(t)),
      Err(_) => Err::<_, Option<T>>(None)
    }
  })
  .join(rx.into_future().then(move |res| {
    let was_canceled = match res {
      Ok((b, _)) => match b {
        Some(b) => b,
        None => false
      },
      Err(_) => return future_ok(None)
    };

    if was_canceled {
      let on_cancel = {
        let mut cancel_guard = on_cancel.write();
        cancel_guard.deref_mut().take()
      };

      if let Some(ft) = on_cancel {
        Box::new(ft.then(|result| {
          match result {
            Ok(t) => future_error(Some(t)),
            Err(_) => future_error(None)
          }
        }))
      }else{
        future_error(None)
      }
    }else{
      future_ok(None)
    }
  }))
  .then(|result: Result<(Option<T>, Option<T>), Option<T>>| {
    match result {
      Ok((ft_res, _)) => Ok(ft_res),
      Err(t) => Ok(t)
    }
  }));

  (out_tx, ft)
}

pub fn is_initialized(id: &Arc<RwLock<Option<usize>>>) -> bool {
  id.read().deref().is_some()
}

pub fn check_initialized(id: &Arc<RwLock<Option<usize>>>) -> Result<(), WomboError> {
  if is_initialized(id) {
    Ok(())
  }else{
    Err(WomboError::new(
      WomboErrorKind::NotInitialized, "Wombo not initialized."
    ))
  }
}

pub fn check_not_initialized(id: &Arc<RwLock<Option<usize>>>) -> Result<(), WomboError> {
  if !is_initialized(id) {
    Ok(())
  }else{
    Err(WomboError::new(
      WomboErrorKind::NotInitialized, "Wombo already initialized."
    ))
  }
}

pub fn get_thread_id(id: &Arc<RwLock<Option<usize>>>) -> Option<usize> {
  let id_guard = id.read();
  id_guard.deref().clone()
}

pub fn set_thread_id(id: &Arc<RwLock<Option<usize>>>) {
  let mut id_guard = id.write();
  let id_ref = id_guard.deref_mut();
  *id_ref = Some(thread_id::get());
}

pub fn clear_thread_id(id: &Arc<RwLock<Option<usize>>>) {
  let mut id_guard = id.write();
  let id_ref = id_guard.deref_mut();
  *id_ref = None;
}

pub fn set_cancel_tx(cancel_tx: &Arc<RwLock<Option<CancelSender>>>, tx: CancelSender) {
  let mut cancel_tx_guard = cancel_tx.write();
  let cancel_tx_ref = cancel_tx_guard.deref_mut();
  *cancel_tx_ref = Some(tx);
}

pub fn take_cancel_tx(cancel_tx: &Arc<RwLock<Option<CancelSender>>>) -> Option<CancelSender> {
  let mut cancel_tx_guard = cancel_tx.write();
  cancel_tx_guard.deref_mut().take()
}

pub fn set_exit_tx<T: Send + 'static>(exit_tx: &Arc<RwLock<Option<ExitSender<T>>>>, tx: ExitSender<T>) {
  let mut exit_tx_guard = exit_tx.write();
  let exit_tx_ref = exit_tx_guard.deref_mut();
  *exit_tx_ref = Some(tx);
}

pub fn take_exit_tx<T: Send + 'static>(exit_tx: &Arc<RwLock<Option<ExitSender<T>>>>) -> Option<ExitSender<T>> {
  let mut exit_tx_guard = exit_tx.write();
  exit_tx_guard.deref_mut().take()
}

#[cfg(test)]
mod tests {
  #![allow(unused_imports)]

  use tokio_core::reactor::{
    Core,
    Handle
  };
  use futures::lazy;

  use super::*;
  use tokio_timer::Timer;
  use std::time::Duration;

  fn fake_callback_ft<T: 'static>(result: T) -> Box<Future<Item=T, Error=()>> {
    Box::new(future::ok::<T, ()>(result))
  }

  #[test]
  fn should_interrupt_timer_without_callback() {
    let mut core = Core::new().unwrap();
    let callbacks = Arc::new(RwLock::new(None));
    let timer = Timer::default();
    let sleep_dur = Duration::from_millis(10000);
    let int_dur = Duration::from_millis(100);

    let timer_ft = Box::new(timer.sleep(sleep_dur)
      .map_err(|_| ())
      .map(|_| 1));
    let (interrupt_tx, test_ft) = interruptible_future(timer_ft, callbacks);

    let interrupt_ft = timer.sleep(int_dur).map_err(|_| ()).and_then(move |_| {
      let _ = interrupt_tx.unbounded_send(true);
      Ok::<_, ()>(())
    });

    match core.run(test_ft.join(interrupt_ft)) {
      Ok((val, _)) => assert_eq!(val, None),
      Err(e) => panic!("Error: {:?}", e)
    }
  }

  #[test]
  fn should_interrupt_timer_with_callback() {
    let mut core = Core::new().unwrap();

    let callback_ft = fake_callback_ft(2);
    let callbacks = Arc::new(RwLock::new(Some(callback_ft)));
    let timer = Timer::default();
    let sleep_dur = Duration::from_millis(10000);
    let int_dur = Duration::from_millis(100);

    let timer_ft = Box::new(timer.sleep(sleep_dur)
      .map_err(|_| ())
      .map(|_| 1));

    let (interrupt_tx, test_ft) = interruptible_future(timer_ft, callbacks);

    let interrupt_ft = timer.sleep(int_dur).map_err(|_| ()).and_then(move |_| {
      let _ = interrupt_tx.unbounded_send(true);
      Ok::<_, ()>(())
    });

    match core.run(test_ft.join(interrupt_ft)) {
      Ok((val, _)) => assert_eq!(val, Some(2)),
      Err(e) => panic!("Error: {:?}", e)
    }
  }

}