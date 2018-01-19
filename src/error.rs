
use std::error::Error;
use std::io::Error as IoError;

use futures::Canceled;
use futures::sync::mpsc::SendError;

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WomboErrorKind {
  /// An error sending a message over an mpsc sender.
  SendError,
  /// See `futures::Canceled`.
  Canceled,
  /// An error indicating the Wombo instance is not initialized.
  NotInitialized,
  /// An unknown error.
  Unknown
}

impl WomboErrorKind {

  pub fn to_string(&self) -> &'static str {
    match *self {
      WomboErrorKind::Canceled       => "Canceled",
      WomboErrorKind::Unknown        => "Unknown",
      WomboErrorKind::SendError      => "Send Error",
      WomboErrorKind::NotInitialized => "Not Initialized"
    }
  }

}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WomboError {
  kind: WomboErrorKind,
  details: String
}

impl WomboError {

  pub fn new<S: Into<String>>(kind: WomboErrorKind, details: S) -> WomboError {
    WomboError { kind, details: details.into() }
  }

  pub fn to_string(&self) -> String {
    format!("{}: {}", self.kind.to_string(), self.details)
  }

  pub fn new_canceled() -> WomboError {
    WomboError {
      kind: WomboErrorKind::Canceled,
      details: String::new()
    }
  }

  pub fn new_not_initialized() -> WomboError {
    WomboError {
      kind: WomboErrorKind::NotInitialized,
      details: String::new()
    }
  }

  pub fn kind(&self) -> &WomboErrorKind {
    &self.kind
  }

}

impl fmt::Display for WomboError {

  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "{}: {}", self.kind.to_string(), self.details)
  }

}

impl Error for WomboError {

  fn description(&self) -> &str {
    &self.details
  }

}

impl From<()> for WomboError {
  fn from(_: ()) -> Self {
    WomboError::new(WomboErrorKind::Unknown, "Empty error.")
  }
}

impl From<Canceled> for WomboError {
  fn from(e: Canceled) -> Self {
    WomboError::new(WomboErrorKind::Canceled, format!("{}", e))
  }
}

impl<T: Into<WomboError>> From<SendError<T>> for WomboError {
  fn from(e: SendError<T>) -> Self {
    WomboError::new(WomboErrorKind::SendError, format!("{}", e))
  }
}

impl From<IoError> for WomboError {
  fn from(e: IoError) -> Self {
    WomboError::new(WomboErrorKind::Unknown, format!("{}", e))
  }
}
