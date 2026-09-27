//! What the 本机设置 page reads back about the operator's own settings.
//!
//! One shape for both directions — the read and the write answer the same
//! question, so a page that just saved does not have to ask again to find out
//! what it saved, and the two can never describe different states.
//!
//! `SearchDirectoryView` is the second shape, and it is separate rather than a
//! wider `SettingsView` because it answers a question the read does not: adding a
//! directory to the search space can evict one, and 「这次挤掉了谁」 is a fact about
//! *that call* which nothing stored can be asked afterwards.
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

/// What adding a directory to the search space did.
///
/// The four facts a person needs to judge the answer, and each is one the page
/// cannot derive:
///
/// - `picked` is the directory they chose — the page never sees it otherwise,
///   because the dialog is opened on the Rust side and only its outcome crosses;
/// - `added` says whether anything changed at all, so 「已经在查找位置里了」 can be
///   said instead of reporting a change that did not happen;
/// - `dropped` names the directory the **bounded** history pushed out, or `null` —
///   a directory dropped here stops being searched, and learning that from a
///   file that quietly got shorter is not learning it;
/// - `searched` is how many directories are searched now, so 「现在共查找 N 个位置」
///   is a number this process measured rather than one the page counted out of a
///   list it was never sent.
///
/// The path is one this process resolved from a dialog it opened, and it is going
/// back to the person who just picked it — the same path the settings file has
/// held since they chose it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SearchDirectoryView {
    pub picked: String,
    pub added: bool,
    pub dropped: Option<String>,
    pub searched: usize,
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

    /// The search-directory answer's keys, pinned for the same reason.
    ///
    /// `dropped` is `null` rather than absent — the page distinguishes 「没有挤掉
    /// 任何一个」 from 「答案里没有这一项」 by reading it, and only the first of those
    /// is a fact this process knows.
    #[test]
    fn the_search_directory_keys_are_pinned() {
        let view = SearchDirectoryView {
            picked: "/Users/operator/Movies/WTMedia-old".to_string(),
            added: true,
            dropped: None,
            searched: 2,
        };
        let value = serde_json::to_value(&view).expect("serialize");

        assert_eq!(
            value
                .as_object()
                .expect("an object")
                .keys()
                .collect::<Vec<_>>(),
            vec!["added", "dropped", "picked", "searched"]
        );
        assert!(value["dropped"].is_null(), "{value}");
        assert_eq!(value["added"], true);
        assert_eq!(value["searched"], 2);
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
