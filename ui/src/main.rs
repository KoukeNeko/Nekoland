//! Every Nekoland widget in one window, in both themes.

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let gallery = Gallery::new()?;
    // The dock and the overview find a window's icon by this; it needs the platform up.
    slint::set_xdg_app_id("nekoland-gallery")?;
    // NEKOLAND_THEME=dark or light pins a theme, for screenshots; the system's otherwise.
    match std::env::var("NEKOLAND_THEME").as_deref() {
        Ok("dark") => gallery.set_dark(true),
        Ok("light") => gallery.set_dark(false),
        _ => {}
    }
    gallery.run()
}
