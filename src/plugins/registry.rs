use std::collections::HashMap;

use super::PluginStep;

/// Registry that stores and retrieves plugins by name
pub struct PluginRegistry {
    plugins: HashMap<String, Box<dyn PluginStep>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Register a plugin. If a plugin with the same name already exists, it is replaced.
    pub fn register(&mut self, plugin: Box<dyn PluginStep>) {
        let name = plugin.name().to_string();
        self.plugins.insert(name, plugin);
    }

    /// Look up a plugin by name
    pub fn get(&self, name: &str) -> Option<&Box<dyn PluginStep>> {
        self.plugins.get(name)
    }

    /// Return the number of registered plugins
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Return true if no plugins are registered
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
