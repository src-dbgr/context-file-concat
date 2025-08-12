use std::sync::Once;

static LOGGING_INIT: Once = Once::new();

/// Initializes the tracing subscriber for tests.
///
/// This function is wrapped in a `Once` block to ensure that the global
/// subscriber is set exactly one time, even when tests are run in parallel.
/// All test modules should call this function at the beginning of their tests.
pub fn setup_test_logging() {
    LOGGING_INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init() // Use try_init() to be safe, though Once makes it redundant.
            .ok(); // Ignore the error if it's already set by another crate.
    });
}
