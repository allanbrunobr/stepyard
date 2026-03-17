use anyhow::{Context as _, Result};
use libloading::Library;

use super::PluginStep;

/// Loads a plugin from a shared library (.so / .dylib) file.
///
/// The library must export a C-ABI function with the signature:
/// ```c
/// extern "C" fn create_plugin() -> *mut dyn PluginStep;
/// ```
pub struct PluginLoader {
    /// Keeps the loaded library alive for as long as the loader exists
    _libraries: Vec<Library>,
}

impl PluginLoader {
    pub fn new() -> Self {
        Self {
            _libraries: Vec::new(),
        }
    }

    /// Load a plugin from the given path.
    ///
    /// # Safety
    /// Loading and calling foreign functions from shared libraries is inherently
    /// unsafe. The caller must ensure the library is a valid minion plugin.
    pub fn load_plugin(path: &str) -> Result<Box<dyn PluginStep>> {
        // SAFETY: We are loading a shared library that is expected to expose
        // a `create_plugin` symbol following the documented ABI.
        unsafe {
            let lib = Library::new(path)
                .with_context(|| format!("Failed to load library at '{path}'"))?;

            let constructor: libloading::Symbol<unsafe extern "C" fn() -> *mut dyn PluginStep> =
                lib.get(b"create_plugin\0").with_context(|| {
                    format!("Library '{path}' does not export 'create_plugin' symbol")
                })?;

            let raw = constructor();
            if raw.is_null() {
                anyhow::bail!("Plugin constructor in '{path}' returned null pointer");
            }

            // Transfer ownership. The Library must outlive the plugin — for
            // production use you would want to keep the Library somewhere. Here
            // we intentionally leak it (acceptable for long-lived plugins) so
            // that the vtable references remain valid.
            std::mem::forget(lib);

            Ok(Box::from_raw(raw))
        }
    }
}

impl Default for PluginLoader {
    fn default() -> Self {
        Self::new()
    }
}
