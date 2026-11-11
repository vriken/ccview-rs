unsafe extern "C" {
    fn tzset();
}

pub(crate) fn local_fields(epoch: i64) -> Option<(i32, u32, u32, u32, u32)> {
    let time = epoch as libc::time_t;
    let mut broken_down: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        tzset();
        if libc::localtime_r(&time, &mut broken_down).is_null() {
            return None;
        }
    }
    Some((
        broken_down.tm_year + 1900,
        (broken_down.tm_mon + 1) as u32,
        broken_down.tm_mday as u32,
        broken_down.tm_hour as u32,
        broken_down.tm_min as u32,
    ))
}
