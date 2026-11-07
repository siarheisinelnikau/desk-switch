pub fn fallback(
    receiving_mouse: bool,
    physical_mouse: bool,
    system_visible: bool,
    touch_suppressed: bool,
) -> bool {
    receiving_mouse && !physical_mouse && !system_visible && !touch_suppressed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pointer_fallback_is_limited_to_an_invisible_remote_mouse_without_local_hardware() {
        assert!(fallback(true, false, false, false));
        assert!(!fallback(false, false, false, false));
        assert!(!fallback(true, true, false, false));
        assert!(!fallback(true, false, true, false));
        assert!(!fallback(true, false, false, true));
    }
}
