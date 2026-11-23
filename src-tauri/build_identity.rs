//! Build-time desktop identity, independent of Cloud routing.
pub fn resolve(
    profile: &str,
    requested: Option<&str>,
    channel: Option<&str>,
) -> Result<&'static str, String> {
    let environment = match requested.unwrap_or(if profile == "debug" { "dev" } else { "prod" }) {
        "dev" => "dev",
        "test" => "test",
        "prod" => "prod",
        _ => return Err("Desktop environment must be dev, test or prod".into()),
    };
    if profile != "debug" && environment == "dev" {
        return Err("release builds require test or prod desktop identity".into());
    }
    if let Some(channel) = channel {
        if channel != "test" && channel != "stable" {
            return Err("unknown desktop release channel".into());
        }
        let expected = if channel == "stable" { "prod" } else { "test" };
        if profile != "release" || environment != expected {
            return Err(format!("{channel} channel requires release + {expected}"));
        }
    }
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn production_gate_never_accepts_test_or_debug() {
        assert_eq!(
            resolve("release", Some("prod"), Some("stable")).unwrap(),
            "prod"
        );
        for (profile, env) in [("debug", "prod"), ("release", "test"), ("release", "dev")] {
            assert!(resolve(profile, Some(env), Some("stable")).is_err());
        }
    }
    #[test]
    fn test_installer_remains_test_in_release_mode() {
        assert_eq!(
            resolve("release", Some("test"), Some("test")).unwrap(),
            "test"
        );
        assert_eq!(resolve("debug", None, None).unwrap(), "dev");
        assert!(resolve("release", Some("typo"), None).is_err());
    }
}
