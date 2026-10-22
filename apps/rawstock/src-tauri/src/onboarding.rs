//! First-launch hook for the optional sample photo. The release build may
//! bundle `samples/onboarding.*` as a resource. Nothing is fetched at
//! runtime and nothing is in the repository.

use std::path::{Path, PathBuf};

pub struct OnboardingSample;

impl OnboardingSample {
    /// The bundled sample, if the build shipped one.
    pub fn find(resource_dir: &Path) -> Option<PathBuf> {
        let dir = resource_dir.join("samples");
        let mut found = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file())
            .filter(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.eq_ignore_ascii_case("onboarding"))
                    && path.extension().is_some()
            })
            .collect::<Vec<_>>();
        found.sort();
        found.into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_is_found_only_when_bundled() {
        let dir = std::env::temp_dir().join(format!("rawstock-onboarding-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(OnboardingSample::find(&dir), None);
        std::fs::create_dir_all(dir.join("samples")).unwrap();
        std::fs::write(dir.join("samples/other.dng"), b"x").unwrap();
        std::fs::write(dir.join("samples/onboarding"), b"x").unwrap();
        assert_eq!(OnboardingSample::find(&dir), None);
        std::fs::write(dir.join("samples/onboarding.dng"), b"x").unwrap();
        assert_eq!(
            OnboardingSample::find(&dir),
            Some(dir.join("samples/onboarding.dng"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
