#[macro_export]
macro_rules! cancellable {
    ($ct:expr, $next:expr) => {
        tokio::select! {
            _ = $ct.cancelled() => {
                break;
            }
            next = $next => {
                next
            }
        }
    };
}