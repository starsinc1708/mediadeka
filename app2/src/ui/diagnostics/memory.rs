#[derive(Clone, Copy)]
pub(super) struct Memory {
    pub(super) working_set: u64,
    pub(super) private: u64,
}

#[cfg(windows)]
pub(super) fn sample() -> Option<Memory> {
    use windows_sys::Win32::System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::GetCurrentProcess,
    };

    let length = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: length,
        ..Default::default()
    };
    // The API selects the extended layout from the supplied size and writes
    // into the caller-owned structure before returning.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::from_mut(&mut counters).cast(),
            length,
        )
    };
    (ok != 0).then_some(Memory {
        working_set: counters.WorkingSetSize as u64,
        private: counters.PrivateUsage as u64,
    })
}

#[cfg(not(windows))]
pub(super) fn sample() -> Option<Memory> {
    None
}
