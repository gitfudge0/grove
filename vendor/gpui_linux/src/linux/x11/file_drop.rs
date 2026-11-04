//! XDND position is a pair of signed, packed root-window coordinates.
pub(super) fn root_position(packed: u32) -> (i16, i16) {
    ((packed >> 16) as i16, packed as i16)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_live_wayland_drag_positions() {
        // Captured GNOME Files -> Grove drag; the independent pointer query
        // remained at the edge while these positions continued to change.
        assert_eq!(root_position(0x05c10524), (1473, 1316));
        assert_eq!(root_position(0x070305c0), (1795, 1472));
    }
    #[test]
    fn preserves_signed_coordinates() {
        assert_eq!(root_position(0xfff00020), (-16, 32));
        assert_eq!(root_position(0x0020fff0), (32, -16));
    }
}

/// ExternalPaths is decoded from URIs, so plain-text path offers are unsuitable.
pub(super) fn is_file_type(offered: u32, uri_list: u32) -> bool {
    offered == uri_list
}

#[cfg(test)]
mod offer_tests {
    use super::*;
    #[test]
    fn requests_uris_when_plain_paths_are_offered_first() {
        let uri_list = 42;
        let offered = [10, 11, uri_list];
        assert_eq!(
            offered
                .into_iter()
                .find(|atom| is_file_type(*atom, uri_list)),
            Some(uri_list)
        );
        assert!(!is_file_type(10, uri_list));
    }
}
