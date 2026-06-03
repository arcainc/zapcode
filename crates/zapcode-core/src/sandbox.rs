use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub memory_limit_bytes: usize,
    pub time_limit_ms: u64,
    pub max_stack_depth: usize,
    pub max_allocations: usize,
    /// Max size of a serialized snapshot/session. A runaway-growth backstop,
    /// kept separate from (and far larger than) `memory_limit_bytes`: callers
    /// typically persist a snapshot to blob/DB storage and pass a pointer
    /// around, so the practical ceiling is generous.
    #[serde(default = "default_max_snapshot_bytes")]
    pub max_snapshot_bytes: usize,
}

fn default_max_snapshot_bytes() -> usize {
    256 * 1024 * 1024 // 256MB
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 32 * 1024 * 1024, // 32MB
            time_limit_ms: 5000,                  // 5s
            max_stack_depth: 512,
            max_allocations: 100_000,
            max_snapshot_bytes: default_max_snapshot_bytes(),
        }
    }
}

impl ResourceLimits {
    /// Clamp each field so it is no looser than the build defaults. Used when
    /// limits are restored from an *untrusted* snapshot/session blob: the SHA in
    /// the wire frame is a keyless integrity check (it detects corruption, not
    /// forgery), so an attacker who controls the stored bytes can otherwise set
    /// arbitrarily large limits and bypass sandbox enforcement on resume. A blob
    /// legitimately produced under *tighter* limits keeps them (we only lower).
    pub fn clamp_to_default(&mut self) {
        let def = ResourceLimits::default();
        self.memory_limit_bytes = self.memory_limit_bytes.min(def.memory_limit_bytes);
        self.time_limit_ms = self.time_limit_ms.min(def.time_limit_ms);
        self.max_stack_depth = self.max_stack_depth.min(def.max_stack_depth);
        self.max_allocations = self.max_allocations.min(def.max_allocations);
        self.max_snapshot_bytes = self.max_snapshot_bytes.min(def.max_snapshot_bytes);
    }
}

/// Tracks resource usage during execution.
#[derive(Debug, Default)]
pub struct ResourceTracker {
    pub allocations: usize,
    pub memory_bytes: usize,
    pub current_stack_depth: usize,
    pub peak_stack_depth: usize,
    start_time: Option<std::time::Instant>,
}

impl ResourceTracker {
    pub fn start(&mut self) {
        self.start_time = Some(std::time::Instant::now());
    }

    pub fn check_time(&self, limits: &ResourceLimits) -> crate::error::Result<()> {
        if let Some(start) = self.start_time {
            if start.elapsed().as_millis() as u64 > limits.time_limit_ms {
                return Err(crate::ZapcodeError::TimeLimitExceeded);
            }
        }
        Ok(())
    }

    pub fn check_stack(&self, limits: &ResourceLimits) -> crate::error::Result<()> {
        if self.current_stack_depth > limits.max_stack_depth {
            return Err(crate::ZapcodeError::StackOverflow(self.current_stack_depth));
        }
        Ok(())
    }

    pub fn track_allocation(&mut self, limits: &ResourceLimits) -> crate::error::Result<()> {
        self.allocations += 1;
        if self.allocations > limits.max_allocations {
            return Err(crate::ZapcodeError::AllocationLimitExceeded);
        }
        Ok(())
    }

    pub fn track_memory(
        &mut self,
        bytes: usize,
        limits: &ResourceLimits,
    ) -> crate::error::Result<()> {
        let next = self.memory_bytes.saturating_add(bytes);
        if next > limits.memory_limit_bytes {
            return Err(crate::ZapcodeError::MemoryLimitExceeded(format!(
                "guest allocation of {} bytes exceeds memory limit of {} bytes",
                bytes, limits.memory_limit_bytes
            )));
        }
        self.memory_bytes = next;
        Ok(())
    }

    pub fn push_frame(&mut self) {
        self.current_stack_depth += 1;
        if self.current_stack_depth > self.peak_stack_depth {
            self.peak_stack_depth = self.current_stack_depth;
        }
    }

    pub fn pop_frame(&mut self) {
        self.current_stack_depth = self.current_stack_depth.saturating_sub(1);
    }
}
