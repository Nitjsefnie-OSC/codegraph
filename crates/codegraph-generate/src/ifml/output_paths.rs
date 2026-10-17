use std::path::PathBuf;

type RoutePageFn = Box<dyn Fn(&str) -> PathBuf + Send + Sync>;
type RouteLoadFn = Box<dyn Fn(&str) -> PathBuf + Send + Sync>;
type NavMapFn = Box<dyn Fn() -> PathBuf + Send + Sync>;
type HelpersFn = Box<dyn Fn() -> PathBuf + Send + Sync>;

pub struct OutputPaths {
    pub route_page: RoutePageFn,
    pub route_load: Option<RouteLoadFn>,
    pub navigation_map: NavMapFn,
    pub route_helpers: Option<HelpersFn>,
    /// The SvelteKit route group the view pages live under (issue #468
    /// W1b integrated mode, e.g. `(views)`); `None` keeps the framework
    /// default `src/routes/` root. A route group is a filesystem-organization
    /// device only — it never appears in URLs — so the navigation map, the
    /// POM routes, and the denial target are unaffected.
    pub route_root: Option<String>,
}

impl OutputPaths {
    pub fn for_framework(framework: &str) -> Self {
        Self::for_framework_with_route_root(framework, None)
    }

    /// Framework paths with an optional SvelteKit route group prefix for the
    /// view pages: `src/routes/(views)/{view}/+page.svelte` in integrated
    /// mode vs the default `src/routes/{view}/+page.svelte`. Only the svelte
    /// layout honors the group; every other framework ignores it (integrated
    /// mode is svelte-only anyway). The `$lib` helpers and the navigation
    /// map are framework-root relative and unchanged by the group.
    pub fn for_framework_with_route_root(framework: &str, route_root: Option<&str>) -> Self {
        let mut paths = Self::for_framework_default(framework);
        if framework == "svelte" {
            let routes_dir = match route_root {
                Some(group) => format!("src/routes/{group}"),
                None => "src/routes".to_string(),
            };
            let load_dir = routes_dir.clone();
            paths.route_page = Box::new(move |name| {
                PathBuf::from(format!("{routes_dir}/{}/+page.svelte", name.to_lowercase()))
            });
            paths.route_load = Some(Box::new(move |name| {
                PathBuf::from(format!("{load_dir}/{}/+page.ts", name.to_lowercase()))
            }));
            paths.route_root = route_root.map(str::to_string);
        }
        paths
    }

    /// The route group the view pages live under, if any.
    pub fn route_root(&self) -> Option<&str> {
        self.route_root.as_deref()
    }

    fn for_framework_default(framework: &str) -> Self {
        match framework {
            "svelte" => Self {
                route_page: Box::new(|name| {
                    PathBuf::from(format!("src/routes/{}/+page.svelte", name.to_lowercase()))
                }),
                route_load: Some(Box::new(|name| {
                    PathBuf::from(format!("src/routes/{}/+page.ts", name.to_lowercase()))
                })),
                navigation_map: Box::new(|| PathBuf::from("src/lib/routes.ts")),
                route_helpers: Some(Box::new(|| PathBuf::from("src/lib/route-helpers.ts"))),
                route_root: None,
            },
            "react" => Self {
                route_page: Box::new(|name| {
                    PathBuf::from(format!(
                        "app/{}/page.tsx",
                        codegraph_naming::to_kebab_case(name)
                    ))
                }),
                route_load: Some(Box::new(|name| {
                    PathBuf::from(format!(
                        "app/{}/page.server.ts",
                        codegraph_naming::to_kebab_case(name)
                    ))
                })),
                navigation_map: Box::new(|| PathBuf::from("src/lib/routes.ts")),
                route_helpers: None,
                route_root: None,
            },
            "vue" => Self {
                route_page: Box::new(|name| {
                    PathBuf::from(format!(
                        "pages/{}.vue",
                        codegraph_naming::to_kebab_case(name)
                    ))
                }),
                route_load: None,
                navigation_map: Box::new(|| PathBuf::from("src/lib/routes.ts")),
                route_helpers: None,
                route_root: None,
            },
            "flutter" => Self {
                route_page: Box::new(|name| {
                    PathBuf::from(format!(
                        "lib/screens/{}_screen.dart",
                        codegraph_naming::to_snake_case(name)
                    ))
                }),
                route_load: None,
                navigation_map: Box::new(|| PathBuf::from("lib/app/routes.dart")),
                route_helpers: None,
                route_root: None,
            },
            "swiftui" => Self {
                route_page: Box::new(|name| PathBuf::from(format!("Views/{}View.swift", name))),
                route_load: None,
                navigation_map: Box::new(|| PathBuf::from("App/Navigation/RouteMap.swift")),
                route_helpers: None,
                route_root: None,
            },
            _ => Self {
                route_page: Box::new(|name| {
                    PathBuf::from(format!("src/routes/{}/+page.svelte", name.to_lowercase()))
                }),
                route_load: Some(Box::new(|name| {
                    PathBuf::from(format!("src/routes/{}/+page.ts", name.to_lowercase()))
                })),
                navigation_map: Box::new(|| PathBuf::from("src/lib/routes.ts")),
                route_helpers: Some(Box::new(|| PathBuf::from("src/lib/route-helpers.ts"))),
                route_root: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_svelte_keeps_the_default_route_root() {
        let paths = OutputPaths::for_framework("svelte");
        assert_eq!(paths.route_root(), None);
        assert_eq!(
            (paths.route_page)("CustomerList"),
            PathBuf::from("src/routes/customerlist/+page.svelte")
        );
        assert_eq!(
            (paths.route_load.unwrap())("CustomerList"),
            PathBuf::from("src/routes/customerlist/+page.ts")
        );
        assert_eq!((paths.navigation_map)(), PathBuf::from("src/lib/routes.ts"));
        assert_eq!(
            (paths.route_helpers.unwrap())(),
            PathBuf::from("src/lib/route-helpers.ts")
        );
    }

    #[test]
    fn integrated_svelte_lands_view_pages_in_the_route_group() {
        let paths = OutputPaths::for_framework_with_route_root("svelte", Some("(views)"));
        assert_eq!(paths.route_root(), Some("(views)"));
        assert_eq!(
            (paths.route_page)("CustomerList"),
            PathBuf::from("src/routes/(views)/customerlist/+page.svelte")
        );
        assert_eq!(
            (paths.route_load.unwrap())("CustomerList"),
            PathBuf::from("src/routes/(views)/customerlist/+page.ts")
        );
        // URL-surface artifacts are group-independent.
        assert_eq!((paths.navigation_map)(), PathBuf::from("src/lib/routes.ts"));
        assert_eq!(
            (paths.route_helpers.unwrap())(),
            PathBuf::from("src/lib/route-helpers.ts")
        );
    }

    #[test]
    fn non_svelte_frameworks_ignore_the_route_root() {
        let paths = OutputPaths::for_framework_with_route_root("react", Some("(views)"));
        assert_eq!(paths.route_root(), None);
        assert_eq!(
            (paths.route_page)("CustomerList"),
            PathBuf::from("app/customer-list/page.tsx")
        );
    }
}
