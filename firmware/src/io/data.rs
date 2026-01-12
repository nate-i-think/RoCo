use crate::io::gps::GNSSFixHandle;

/// RoCo most systems' I/O runs off of this `AppContext` struct, which
/// basically does dependancy injection via a shared context struct for
/// all system data. This provides everyone with references to Nav Data,
/// Accelorometer Data, etc.
#[derive(Clone)]
pub struct AppContext {
    gnss_fix_handle: GNSSFixHandle,
}