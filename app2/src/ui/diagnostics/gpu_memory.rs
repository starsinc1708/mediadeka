#[cfg(windows)]
pub(super) fn dedicated_bytes() -> Option<u64> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::Performance::{
        PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_LARGE,
        PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
        PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };

    const MAX_BUFFER_BYTES: u32 = 1024 * 1024;
    struct Query(PDH_HQUERY);
    impl Drop for Query {
        fn drop(&mut self) {
            // Closing the query also releases its counter.
            unsafe { PdhCloseQuery(self.0) };
        }
    }

    let mut raw = null_mut();
    // Null data source selects live counters on this machine.
    if unsafe { PdhOpenQueryW(null(), 0, &mut raw) } != 0 {
        return None;
    }
    let query = Query(raw);
    let path = "\\GPU Process Memory(*)\\Dedicated Usage\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let mut counter = null_mut();
    // The counter path is NUL-terminated and stays valid through the call.
    if unsafe { PdhAddEnglishCounterW(query.0, path.as_ptr(), 0, &mut counter) } != 0
        || unsafe { PdhCollectQueryData(query.0) } != 0
    {
        return None;
    }

    let mut bytes = 0;
    let mut count = 0;
    // PDH reports the allocation size on the first call, including the names.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut bytes, &mut count, null_mut())
    };
    if status != PDH_MORE_DATA || bytes == 0 || bytes > MAX_BUFFER_BYTES {
        return None;
    }
    let slots = (bytes as usize).div_ceil(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>());
    let mut buffer = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); slots];
    // The slot type supplies the alignment and the allocation covers the requested size.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_LARGE,
            &mut bytes,
            &mut count,
            buffer.as_mut_ptr(),
        )
    };
    if status != 0 {
        return None;
    }
    let prefix = format!("pid_{}_", std::process::id())
        .encode_utf16()
        .collect::<Vec<_>>();
    let mut total: Option<u64> = None;
    for item in buffer.get(..count as usize)? {
        let valid = matches!(
            item.FmtValue.CStatus,
            PDH_CSTATUS_NEW_DATA | PDH_CSTATUS_VALID_DATA
        );
        // A successful read returns NUL-terminated names in the live buffer.
        // Stop at the first difference, including an early terminator.
        let same_process = !item.szName.is_null()
            && prefix
                .iter()
                .enumerate()
                .all(|(index, unit)| (unsafe { *item.szName.add(index) }) == *unit);
        // PDH_FMT_LARGE selects the signed 64-bit union member.
        if valid
            && same_process
            && let Ok(value) = u64::try_from(unsafe { item.FmtValue.Anonymous.largeValue })
        {
            total = Some(total.unwrap_or_default().saturating_add(value));
        }
    }
    total
}

#[cfg(not(windows))]
pub(super) fn dedicated_bytes() -> Option<u64> {
    None
}
