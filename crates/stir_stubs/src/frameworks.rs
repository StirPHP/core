//! What StirPHP knows of frameworks, shipped inside the binary: a Rhai script for each, using the
//! same API as a project's own scripts, and stub files laid over the framework's declarations where
//! its PHPDoc says less than it could.
//!
//! These are ours, so unlike a project's scripts they need nobody's say-so to run. They're never
//! read from disk, and their paths are under `stir_db::BUNDLED_PREFIX`, which no file of a project
//! can be.

/// Support for one framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Framework {
    /// As `parameters.stirphp.frameworks` names it.
    pub name: &'static str,
    /// The script's file name, under `stir_db::BUNDLED_PREFIX`.
    pub script_name: &'static str,
    pub script: &'static str,
    /// Stub files, by their path under `stubs://`: under `override/`, what's laid over the
    /// framework's own declarations, and elsewhere what's declared as it is.
    pub stubs: &'static [(&'static str, &'static str)],
}

/// Every framework there's support for.
pub const FRAMEWORKS: &[Framework] = &[
    Framework {
        name: "laravel",
        script_name: "laravel.rhai",
        script: include_str!("../frameworks/laravel.rhai"),
        stubs: &[
            (
                "frameworks/laravel/aliases.php",
                include_str!("../frameworks/laravel/aliases.php"),
            ),
            (
                "frameworks/laravel/blade.php",
                include_str!("../frameworks/laravel/blade.php"),
            ),
            (
                "override/frameworks/laravel/EloquentBuilder.stub",
                include_str!("../frameworks/laravel/EloquentBuilder.stub"),
            ),
            (
                "override/frameworks/laravel/ConsoleKernel.stub",
                include_str!("../frameworks/laravel/ConsoleKernel.stub"),
            ),
        ],
    },
    Framework {
        name: "pest",
        script_name: "pest.rhai",
        script: include_str!("../frameworks/pest.rhai"),
        stubs: &[(
            "override/frameworks/pest/Hooks.stub",
            include_str!("../frameworks/pest/Hooks.stub"),
        )],
    },
    Framework {
        name: "symfony",
        script_name: "symfony.rhai",
        script: include_str!("../frameworks/symfony.rhai"),
        stubs: &[],
    },
    Framework {
        name: "wordpress",
        script_name: "wordpress.rhai",
        script: include_str!("../frameworks/wordpress.rhai"),
        stubs: &[],
    },
];

/// The support for the framework named `name`.
pub fn framework(name: &str) -> Option<&'static Framework> {
    FRAMEWORKS.iter().find(|f| f.name == name)
}
