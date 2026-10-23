//! The input-method and virtual-keyboard protocols labwc 0.20 speaks.
//! The XML is the copy shipped with wlroots 0.20.2.

#[allow(dead_code, unused_imports, clippy::all)]
pub mod input_method {
    use wayland_client;
    use wayland_client::protocol::*;
    pub use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocol/input-method-unstable-v2.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocol/input-method-unstable-v2.xml");
}

#[allow(dead_code, unused_imports, clippy::all)]
pub mod virtual_keyboard {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocol/virtual-keyboard-unstable-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocol/virtual-keyboard-unstable-v1.xml");
}
