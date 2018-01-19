Wombo
=====

[![Build Status](https://travis-ci.org/azuqua/wombo.rs.svg?branch=master)](https://travis-ci.org/azuqua/wombo.rs)
[![Crates.io](https://img.shields.io/crates/v/wombo.svg)](https://crates.io/crates/wombo)

[Documentation](https://docs.rs/wombo/*/wombo/)

Utilities for managing event loop threads.

## Install
 
With [cargo edit](https://github.com/killercup/cargo-edit).
 
```
cargo add wombo
```

## Features

* Less boilerplate spawning event loop threads.
* Cancel event loops, with an interface to clean up resources on the event loop first.
* Receive notifications when event loop threads exit, with arbitrary data.

## Usage

Spawn an event loop thread that waits for a second, and conditionally interrupt it after 50 ms, returning some data to the main thread.

```rust

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

```

See [examples](examples/README.md) for more.

## Tests

To run the tests:

```
cargo test
```
