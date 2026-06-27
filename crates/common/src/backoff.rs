use std::time::Duration;

/// Exponential backoff (500ms base, 5s cap). Used by keeper, liquidator,
/// mm-bot, and api watcher so the behavior is identical across all services.
pub struct Backoff {
    current: Duration,
}

impl Backoff {
    const BASE: Duration = Duration::from_millis(500);
    const MAX: Duration = Duration::from_secs(5);

    pub fn new() -> Self {
        Self {
            current: Self::BASE,
        }
    }

    pub fn reset(&mut self) {
        self.current = Self::BASE;
    }

    pub async fn sleep(&mut self) {
        tokio::time::sleep(self.current).await;
        self.current = (self.current * 2).min(Self::MAX);
    }
}
