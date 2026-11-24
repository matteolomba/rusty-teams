use std::sync::OnceLock;

static BUILD_VERSION: OnceLock<&'static str> = OnceLock::new();

pub fn set_build_version(version: &'static str) {
    let _ = BUILD_VERSION.set(version);
}

pub fn build_version() -> &'static str {
    BUILD_VERSION.get().copied().unwrap_or("unknown")
}

static RELEASE_TAG: OnceLock<&'static str> = OnceLock::new();

pub fn set_release_tag(tag: &'static str) {
    let _ = RELEASE_TAG.set(tag);
}

/// Set only in builds of a `v*` tag, so local and deployed dev builds never pull a GitHub release.
pub fn release_tag() -> Option<&'static str> {
    RELEASE_TAG.get().copied()
}

/// The release tag this build represents. CI tag builds report their tag; local builds
/// report the workspace version. Unlike `build_version()` (commit + timestamp) this is
/// comparable with release asset tags.
pub fn running_release_tag() -> &'static str {
    release_tag().unwrap_or(concat!("v", env!("CARGO_PKG_VERSION")))
}
