use std::time::Instant;

pub(crate) struct Clock {
    started: Instant,
    test_wall_ms: Option<i64>,
    #[cfg(feature = "test-harness")]
    test_offset_ms: std::sync::atomic::AtomicI64,
}
impl Clock {
    pub fn new(test_wall_ms: Option<i64>) -> Self {
        Self {
            started: Instant::now(),
            test_wall_ms,
            #[cfg(feature = "test-harness")]
            test_offset_ms: std::sync::atomic::AtomicI64::new(0),
        }
    }
    pub fn now_ms(&self) -> i64 {
        if let Some(base) = self.test_wall_ms {
            #[cfg(feature = "test-harness")]
            let base = base.saturating_add(
                self.test_offset_ms
                    .load(std::sync::atomic::Ordering::Relaxed),
            );
            base.saturating_add(
                i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX),
            )
        } else {
            chrono::Utc::now().timestamp_millis()
        }
    }
    #[cfg(feature = "test-harness")]
    pub fn set_test_offset(&self, offset: i64) {
        self.test_offset_ms
            .store(offset, std::sync::atomic::Ordering::Relaxed);
    }
    pub async fn wait_until(&self, wall_ms: i64) {
        let mut previous_wall = self.now_ms();
        let mut previous_tick = tokio::time::Instant::now();
        let mut deadline = previous_tick.checked_add(std::time::Duration::from_millis(
            wall_ms.saturating_sub(previous_wall).max(0) as u64,
        ));
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let now = self.now_ms();
            let instant = tokio::time::Instant::now();
            let elapsed = i64::try_from(instant.duration_since(previous_tick).as_millis())
                .unwrap_or(i64::MAX);
            if now.saturating_sub(previous_wall) > elapsed.saturating_add(1000) {
                deadline = instant.checked_add(std::time::Duration::from_millis(
                    wall_ms.saturating_sub(now).max(0) as u64,
                ));
            }
            if deadline.is_some_and(|at| at <= instant) {
                return;
            }
            previous_wall = now;
            previous_tick = instant;
        }
    }
}
