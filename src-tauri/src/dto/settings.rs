//! What the 本机设置 page reads back about the operator's own settings.
//!
//! One shape for both directions — the read and the write answer the same
//! question, so a page that just saved does not have to ask again to find out
//! what it saved, and the two can never describe different states.
//!
//! Two fields, and each is one a person acts on. `save_dir` is the choice
//! itself, `None` when nothing has been chosen; the page says 「未设置」for that
//! rather than inventing a default, because the default is the *task's* decision
//! (`settings::UserSettings::save_dir`) and not something this component can
//! truthfully state. `file` is where the choice is kept, and it is on the wire
//! for the one case that needs it: a settings file this build cannot read is left
//! exactly as it is, so the person reading the error is the one who has to find
//! it.
//!
//! The path is a path this process resolved, never one a caller supplied, and it
//! carries no user content — a settings *value* is the one thing this module
//! could have echoed back and does not.
//!
//! Field names are load-bearing: the Vue layer reads them by name, and
//! `SettingsView`'s key set is pinned by the test at the bottom rather than left
//! to be discovered by a page that silently shows `undefined`.

use serde::Serialize;

/// The operator's settings, as the page reads and writes them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SettingsView {
    /// Where downloaded material and finished pieces are saved, or `None` when
    /// the operator has not chosen. Written as an absent key rather than as an
    /// empty string: 「没有选择」and 「选择了空路径」are different answers.
    pub save_dir: Option<String>,
    /// The settings file, absolute — `settings::path` under the data root.
    pub file: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key set, pinned. A rename here is a page that shows nothing, and the
    /// Vue layer cannot be asked about it at build time.
    #[test]
    fn the_wire_key_sets_are_pinned() {
        let view = SettingsView {
            save_dir: Some("/Users/operator/Movies".to_string()),
            file: "/Users/operator/Library/Application Support/WTMedia/Desktop/settings.toml"
                .to_string(),
        };
        let value = serde_json::to_value(&view).expect("serialize");

        assert_eq!(
            value
                .as_object()
                .expect("an object")
                .keys()
                .collect::<Vec<_>>(),
            vec!["file", "save_dir"]
        );
        assert_eq!(value["save_dir"], "/Users/operator/Movies");
    }

    /// An unchosen directory is `null`, not `""`.
    ///
    /// The two are not the same state, and a page that rendered the empty string
    /// would show a box that looks filled in. The decision is `settings`'s
    /// (`skip_serializing_if` on the struct); this is the statement that it
    /// survives the trip through JSON, which is what the page actually sees.
    #[test]
    fn an_unchosen_directory_arrives_as_null() {
        let view = SettingsView {
            save_dir: None,
            file: "/tmp/settings.toml".to_string(),
        };
        let value = serde_json::to_value(&view).expect("serialize");

        assert!(value["save_dir"].is_null(), "{value}");
        // The other field is still there: a null `save_dir` must not take the
        // file's location away with it.
        assert_eq!(value["file"], "/tmp/settings.toml");
    }
}
