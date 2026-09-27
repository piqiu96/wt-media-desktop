//! Where downloads land: what the Agent says about the folder it was given.
//!
//! One shape, and it crosses this boundary **twice**: the Agent answers with it
//! (CHG-061 T-04's `POST /api/v1/save-directory`), and the command hands it
//! straight to the page. That is deliberate rather than lazy — the two facts
//! beside the directory are *measurements of this machine at the moment it was
//! asked* (`writable`, `free_bytes`), so a page that showed its own copy would be
//! showing when the operator picked the folder rather than whether a download
//! could land in it now.
//!
//! This is also why the two are not `Option`: the Agent answers 「读不出来」for a
//! directory it cannot measure by reporting `writable: false` with
//! `free_bytes: 0` (its `_save_directory_facts`), so a missing number here would
//! mean the answer never arrived rather than that it could not be taken — and
//! that is a failed command, not a `null` field.
//!
//! Field names are load-bearing in both directions: the envelope is what serde
//! decodes the Agent's body into, and the inner key set is pinned by the test at
//! the bottom so a rename on either side cannot be discovered by a page that
//! shows `undefined`.

use serde::{Deserialize, Serialize};

/// The Agent's `{"data": …}` envelope, as `LocalAgentStatusResponse` is for the
/// status route. A second wrapper rather than a generic one: `data` is the whole
/// of this route's answer, and a generic envelope would invite a caller to read a
/// `message` or an `errcode` this route never writes.
#[derive(Clone, Debug, Deserialize)]
pub struct SaveDirectoryResponse {
    pub data: SaveDirectoryFacts,
}

/// The stored save directory and the facts about it, read when it was asked for.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SaveDirectoryFacts {
    /// The directory the **Agent** has stored, or `None` when none is.
    ///
    /// Read back from the Agent rather than echoed from Desktop's own settings
    /// file, and the two can disagree: the operator may have chosen a folder
    /// while the Agent was not running, so what this field reports is 「Agent 现在
    /// 会往哪里写」— which is the question a download depends on.
    pub save_dir: Option<String>,
    /// Whether a task could write there right now.
    pub writable: bool,
    /// Bytes free on that volume, clamped at zero by the Agent.
    pub free_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key set, pinned, and the values it decodes from.
    ///
    /// The Agent's spelling is snake_case and these are the three words its
    /// `_save_directory_facts` writes; a rename on either side would make a
    /// download land somewhere the page does not name.
    #[test]
    fn the_wire_key_set_is_the_agents_and_is_pinned() {
        let answer = r#"{"data":{"save_dir":"/Volumes/Movies","writable":true,"free_bytes":123}}"#;

        let response: SaveDirectoryResponse =
            serde_json::from_str(answer).expect("the Agent's own shape must decode");

        assert_eq!(
            response.data,
            SaveDirectoryFacts {
                save_dir: Some("/Volumes/Movies".to_string()),
                writable: true,
                free_bytes: 123,
            }
        );
        assert_eq!(
            serde_json::to_value(&response.data)
                .expect("serialize")
                .as_object()
                .expect("an object")
                .keys()
                .collect::<Vec<_>>(),
            vec!["free_bytes", "save_dir", "writable"]
        );
    }

    /// 「没有选择」and 「选择了空路径」are two answers, and only one of them is a
    /// directory.
    #[test]
    fn an_unchosen_directory_is_null_and_not_the_empty_string() {
        let response: SaveDirectoryResponse =
            serde_json::from_str(r#"{"data":{"save_dir":null,"writable":false,"free_bytes":0}}"#)
                .expect("the Agent's own shape must decode");

        assert_eq!(response.data.save_dir, None);
        assert!(
            !response.data.writable,
            "nothing is chosen, so nothing is writable"
        );
    }
}
