/// A route: a path prefix and the handler name it dispatches to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub prefix: String,
    pub handler: String,
}

/// A routing table, checked in the order routes were added.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Router {
    routes: Vec<Route>,
}

impl Router {
    /// Append a route; earlier routes take precedence over later ones.
    pub fn add(&mut self, prefix: &str, handler: &str) {
        self.routes.push(Route { prefix: prefix.to_string(), handler: handler.to_string() });
    }

    /// The handler for `path`: the first route, in insertion order, whose
    /// prefix `path` starts with. `None` when no route matches.
    pub fn resolve(&self, path: &str) -> Option<&str> {
        self.routes
            .iter()
            .filter(|route| path.starts_with(route.prefix.as_str()))
            .last()
            .map(|route| route.handler.as_str())
    }

    /// How many routes are registered.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether no routes are registered.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}
