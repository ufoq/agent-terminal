mod list;
mod press;
mod read;
mod send;
mod start;
mod stop;

use crate::error::Error;

/// A dispatched input (paste/send-keys) that timed out after reaching the pane
/// is inherently ambiguous: the input may or may not have landed. Surface that
/// as `delivery_uncertain` so the caller reads before resending, rather than
/// as a backend failure.
fn dispatch(result: Result<(), Error>) -> Result<(), Error> {
    match result {
        Ok(()) => Ok(()),
        Err(Error::ZellijTimeout) => Err(Error::DeliveryUncertain),
        Err(error) => Err(error),
    }
}
