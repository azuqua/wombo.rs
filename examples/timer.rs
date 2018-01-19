#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_variables)]

extern crate futures;
extern crate wombo;
extern crate tokio_timer;

use futures::Future;
use tokio_timer::Timer;

use std::thread;
use std::time::Duration;

use wombo::*;

const SHOULD_INTERRUPT: bool = true;

fn main() {
  // expected value when not interrupted
  let foo = 1;
  // expected value when interrupted
  let bar = 2;

  let timer = Timer::default();
  let sleep_dur = Duration::from_millis(50);

  let wombo = Wombo::new();
  let options = ThreadOptions::new("loop-1", None);

  let spawn_result = wombo.spawn(options, move |handle, hooks| {
    // sleep for a second, then return foo, or if canceled return bar
    let dur = Duration::from_millis(1000);

    hooks.on_cancel(move || Ok(bar));

    timer.sleep(dur)
      .map_err(|_| ())
      .and_then(move |_| Ok(foo))
  });

  if let Err(e) = spawn_result {
    panic!("Error spawning event loop thread: {:?}", e);
  }

  // give the child thread a chance to initialize
  thread::sleep(sleep_dur);

  assert!(wombo.is_running());
  println!("Wombo {} running thread {:?}", wombo.id(), wombo.core_thread_id());

  if SHOULD_INTERRUPT {
    if let Err(e) = wombo.cancel() {
      panic!("Error canceling: {:?}", e);
    }
  }

  let result = match wombo.on_exit() {
    Ok(rx) => rx.wait().unwrap(),
    Err(e) => panic!("Error calling on_exit: {:?}", e)
  };

  if SHOULD_INTERRUPT {
    assert_eq!(result, Some(bar));
  }else{
    assert_eq!(result, Some(foo));
  }
}