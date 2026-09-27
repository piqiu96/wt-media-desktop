//! Where a download lands: the operator's choice, handed to the Local Agent.
//!
//! One fact, two owners, and this module is the seam. The operator chooses the
//! folder on the 本机设置 page and Desktop stores it in its own settings file
//! (`commands::settings`); the **Agent** is what writes the bytes, so it has to
//! be told, and its own copy is what the download executor reads. Neither side
//! can derive the other's value, so the choice travels over the Agent's
//! save-directory route — the shape of the channel CHG-061 T-04 settled on
//! (「新增 Local Agent 本机 API 端点」), not a second command path: the Agent's
//! server checks the runtime token on **every** route, so a push over the same
//! loopback HTTP the bind flow uses is authenticated by construction
//! (`local_api/server.py::_check_auth`, no per-route exception).
//!
//! ## Why the push is its own function and not part of a settings write
//!
//! A settings write is local and a push is a network call, and entangling them
//! would make 「保存失败」mean either 「这台机器的设置文件写不进去」or 「Agent 没在
//! 运行」 — two sentences the operator has to act on differently. So
//! `local_settings_set` stores and nothing else; the push happens here, and a
//! push that fails leaves the choice stored, which is the state the operator can
//! retry from.
//!
//! ## Why a start re-pushes what it already pushed
//!
//! The Agent is a child process of this one and keeps no memory across a restart:
//! the node credential is held **in memory only** (T-04's ruling) and the save
//! directory is in the Agent's own SQLite, which can be older than the choice on
//! disk. A `local_agent_start` that succeeded is therefore the one moment where
//! saying both again is cheap and correct, and the two `offer_*` functions below
//! are that moment's whole content. They are best-effort **and separate**: the
//! Agent is up and healthy by the time they run, so a push that fails must not
//! turn a start that plainly worked into an error, and the credential failing
//! must not stop the directory from being offered.
//!
//! Both are split out of the start for the reason the record functions in
//! `commands::agent` are: a command takes `State`, which no test can build, and
//! a function nobody can call is a function nobody can check.

use crate::dto::{LocalErrorBody, SaveDirectoryFacts, SaveDirectoryResponse, SettingsView};
use crate::http::LocalAgentClient;
use crate::state::RuntimeBinding;
use std::path::{Path, PathBuf};
use tauri::State;
use tauri_plugin_dialog::{DialogExt, FilePath};

use super::{bind, settings};

/// What Desktop posts to the Agent's save-directory route.
///
/// One key, and it is the Agent's spelling: `local_api/server.py::
/// set_save_directory_response` reads `body.get("save_dir")` and answers a
/// `save_directory_invalid` to anything that is not a string — so a rename here
/// is not a cosmetic change, it is a 400 that reads like a bad path.
pub(crate) fn save_directory_body(save_dir: &str) -> serde_json::Value {
    serde_json::json!({ "save_dir": save_dir })
}

/// Hand the Agent the folder to write downloads into, and read back its answer.
///
/// The answer is the Agent's **stored** facts rather than an echo of the request
/// (`set_save_directory_response` reads back), and it is returned rather than
/// discarded because the two facts beside the directory — `writable`,
/// `free_bytes` — are measurements the page would otherwise have to guess at.
///
/// The path is deliberately **not** in any error message. It is this machine's
/// filesystem layout, CHG-061 §4 keeps local paths out of records, and the
/// caller here records what it returns (`commands::agent::offer_failed`) — so a
/// message built with `format!("…{save_dir}…")` would be the one line that puts
/// the operator's home directory in the log file. The Agent's own refusals
/// already carry no path for the same reason; this keeps Desktop's side of the
/// same rule.
pub(crate) async fn push(
    client: &crate::http::LocalAgentClient,
    save_dir: &str,
) -> Result<SaveDirectoryFacts, String> {
    let response = client
        .post("/api/v1/save-directory")
        .json(&save_directory_body(save_dir))
        .send()
        .await
        .map_err(|e| format!("写入Local Agent保存目录失败: {}", e))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(match LocalErrorBody::code_of(&text) {
            Some(code) => format!("写入Local Agent保存目录失败: {}（{}）", status, code),
            None => format!("写入Local Agent保存目录失败: {}", status),
        });
    }
    let answer: SaveDirectoryResponse = serde_json::from_str(&text)
        .map_err(|e| format!("解析Local Agent保存目录响应失败: {}", e))?;
    Ok(answer.data)
}

/// Tell a freshly started Agent the Cloud identity it has to report under.
///
/// `None` when there is nothing to say or it was said: no binding means this
/// Desktop has not been bound in this launch, and the credential that would be
/// pushed is one Cloud issues at bind time — Desktop holds nothing to re-state.
/// `Some(reason)` only when an attempt was made and failed, which is the case
/// worth a record: the Agent is running with no way to report a download.
///
/// The push itself is `bind::bind_node`, the **same** function the bind flow
/// calls. A second implementation would be the one that drifted, and the drift
/// would be silent — the Agent would answer 200 to both and only the download
/// would notice.
pub(crate) async fn offer_credential(
    client: &crate::http::LocalAgentClient,
    binding: Option<&RuntimeBinding>,
) -> Option<String> {
    let held = binding?;
    bind::bind_node(client, &held.node_id, &held.node_credential)
        .await
        .err()
}

/// Tell a freshly started Agent which folder this machine's downloads go in.
///
/// `None` when the choice was pushed, or when there is no choice to push.
///
/// **「没有选择」is not pushed as a clearing**, because the route cannot express
/// one: its body is a `save_dir` string, and the Agent answers a
/// `save_directory_invalid` to a missing or `null` value rather than treating it
/// as 「忘了那个目录」. So an operator who clears the choice leaves the Agent on
/// the folder it was last given, and downloads keep landing there until a new
/// one is chosen. Registered in CHG-061's checkpoint rather than worked around
/// here: teaching the route to clear is a change to the frozen Local Agent
/// contract, which is a decision and not a debugging step.
pub(crate) async fn offer_directory(
    client: &crate::http::LocalAgentClient,
    chosen: Option<&str>,
) -> Option<String> {
    push(client, chosen?).await.err()
}

/// The whole of what a start says to the Agent that just came up, and what it
/// could not say.
///
/// The composition, split out of `commands::agent::start` for the reason every
/// other body there is: `start` takes an `AppHandle` and reaches its success arm
/// only against a real Agent, so a sequence written inside it would be reachable
/// by no test at all — and the property this function exists for is exactly a
/// property of the sequence. **A failure is collected, never propagated**: an
/// Agent that is up is what the caller asked for, and one push failing must not
/// stop the other from being attempted.
///
/// `chosen` is a `Result` rather than an `Option` because reading the operator's
/// settings file can fail, and that failure is one of the things a start has to
/// report: it is a different sentence from 「没有选择」 and from 「Agent 拒绝了」,
/// and all three end with a download that will not run. Read by the caller
/// (`settings::chosen_save_dir`) rather than here, so this function is a function
/// of its arguments — no file, no layout, and a test that can produce all four
/// cases.
pub(crate) async fn offer_stored_choices(
    client: &crate::http::LocalAgentClient,
    binding: Option<&RuntimeBinding>,
    chosen: Result<Option<String>, String>,
) -> Vec<String> {
    let mut failures = Vec::new();
    if let Some(reason) = offer_credential(client, binding).await {
        failures.push(reason);
    }
    match chosen {
        Ok(chosen) => {
            if let Some(reason) = offer_directory(client, chosen.as_deref()).await {
                failures.push(reason);
            }
        }
        Err(reason) => failures.push(reason),
    }
    failures
}

/// The directory a picked `FilePath` names, or why it does not name one.
///
/// Split out of the command because everything the picker's answer is judged by
/// is here, and the two arms are both reachable: `into_path` converts the
/// `file://` form some platforms answer with, and fails for a URL that is not a
/// file at all — a `content://` document, an `https://` share. The call to
/// `settings::set_save_dir` that follows is the one line a test cannot make:
/// it writes through the **real** data root, which is why `store_chosen` used to
/// be untestable in its happy arm.
///
/// `Ok(None)` is a **cancelled dialog**, not a failure: the operator closing the
/// picker is a decision, and a page that showed an error for it would be
/// reporting their own click back as a fault.
fn chosen_path(chosen: Option<FilePath>) -> Result<Option<PathBuf>, String> {
    let Some(file_path) = chosen else {
        return Ok(None);
    };
    file_path
        .into_path()
        .map(Some)
        .map_err(|error| format!("这个位置不是本机目录，无法作为保存位置：{error}"))
}

/// Everything the push command does, as a function of what it read.
///
/// `chosen` is passed in rather than read here so the three answers a page can
/// get — 「没有选择」/「设置文件读不出来」/「Agent 的答复」— are all reachable from
/// a test; `settings::chosen_save_dir` needs a real data root, which a test must
/// not have.
async fn push_chosen(
    client: &LocalAgentClient,
    chosen: Result<Option<String>, String>,
) -> Result<SaveDirectoryFacts, String> {
    let chosen = chosen?;
    // Not a `None`-shaped success: the page asked to push a choice, and there is
    // none. `offer_directory`'s no-op arm is for a *start*, where the answer is
    // not being waited for; here somebody clicked and deserves a sentence.
    let directory = chosen.ok_or_else(|| "还没有选择下载保存位置".to_string())?;
    push(client, &directory).await
}

/// A file name that may be looked up in the save directory, or why it may not.
///
/// The page sends a **name**, never a path, and this is where that is enforced —
/// the same move as `local_log_tail`'s 「列表即白名单」. `Path::file_name` is the
/// check rather than a scan for separators: a name is one component, so anything
/// that is not exactly one component is refused whether the separator is `/`,
/// `\`, a leading `~`, or nothing at all. `.` and `..` are refused by the same
/// rule rather than by a case for them: `file_name` answers `None` for both.
///
/// **Byte for byte, with no trim.** A trimmed name would be a file the download
/// list shows and this command cannot open — the names the page sends come from
/// the task records, not from a keyboard, so there is nothing to be lenient
/// about, and `every_name_in_a_listing_is_looked_up_as_it_is_spelled` is the
/// case that holds that.
fn file_name_of(name: &str) -> Result<&str, String> {
    match Path::new(name).file_name() {
        Some(component) if component == std::ffi::OsStr::new(name) => Ok(name),
        _ => Err(format!(
            "{name:?} 不是一个文件名（只接受保存目录里的名字，不接受路径）"
        )),
    }
}

/// Find `name` in `directory`, or say why it is not there.
///
/// The directory is **listed** and the name matched against what is in it, rather
/// than joined onto the directory and passed to the opener: joining would make
/// any name the page could spell a path this process opens, and the two are only
/// distinguishable at the moment of the check. `file_name_of` refuses a
/// path-shaped name first, which is what keeps that property if the lookup below
/// is ever rewritten as a join — a `join` plus `is_file` looks like the same
/// check and is not one.
///
/// A directory that shares the name is not a download, so the match requires a
/// regular file. `DirEntry::file_type` does not follow symlinks, so a symlink
/// planted in the save directory is not one either — a download this app wrote
/// is a file, and following a link is how 「打开下载的文件」 turns into 「打开链接
/// 指向的任何东西」.
///
/// Returns the path rather than opening it, so every decision above is a test's
/// and the opener is the command's one untestable line — the split
/// `reveal`'s `reveal` draws.
fn find_saved_file(directory: &Path, name: &str) -> Result<PathBuf, String> {
    let wanted = file_name_of(name)?;
    look_for(directory, wanted).map_err(|miss| match miss {
        Lookup::Absent => format!(
            "{} 里没有 {wanted}：可能已被移动或删除",
            directory.display()
        ),
        Lookup::Unreadable(sentence) => sentence,
    })
}

/// Why one directory did not produce the file.
///
/// The two are kept apart because they are different answers about **different
/// things**: `Absent` is about the file, `Unreadable` is about the directory.
/// Collapsing them is what makes 「文件不在」 the report for a folder nobody
/// could open — and a person told 「已删除」 re-downloads material they still have.
enum Lookup {
    /// The directory was read and the name is not in it.
    Absent,
    /// The directory could not be looked in, so the file may well be inside it.
    /// Held as the sentence, because the two ways it fails (not a directory any
    /// more, cannot be listed) say different things to a person.
    Unreadable(String),
}

/// The lookup itself, without deciding what its failure means.
fn look_for(directory: &Path, wanted: &str) -> Result<PathBuf, Lookup> {
    if !directory.is_dir() {
        return Err(Lookup::Unreadable(format!(
            "保存位置 {} 现在不是一个目录：到本机设置里重新选一个",
            directory.display()
        )));
    }
    std::fs::read_dir(directory)
        .map_err(|error| {
            Lookup::Unreadable(format!("无法读取保存位置 {}：{error}", directory.display()))
        })?
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_name().to_str() == Some(wanted)
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .map(|entry| entry.path())
        .ok_or(Lookup::Absent)
}

/// Find `name` in every directory this machine has written downloads into.
///
/// Newest first, and the history is not a courtesy: the reason it exists is that
/// a file downloaded while an older directory was current is **still there**
/// afterwards, and a lookup that only knew the directory in use now would report
/// it as 「可能已被移动或删除」 — which is the defect this exists to remove. So
/// the first directory that has the file wins, wherever it is in the list.
///
/// A directory that could not be read is not an error by itself: the file may be
/// in the next one, and failing on it would refuse to open a file that is
/// sitting right there. It is only *reported* — in the message, when nothing was
/// found — because 「这个文件不在本机」 and 「有三个地方我没能查」 are different
/// answers and only one of them is about the file.
fn find_in_known(dirs: &[PathBuf], name: &str) -> Result<PathBuf, String> {
    let wanted = file_name_of(name)?;
    if dirs.is_empty() {
        return Err("还没有选择下载保存位置".to_string());
    }
    let mut searched: Vec<String> = Vec::with_capacity(dirs.len());
    let mut unreadable: Vec<String> = Vec::new();
    for directory in dirs {
        match look_for(directory, wanted) {
            Ok(found) => return Ok(found),
            Err(Lookup::Absent) => searched.push(directory.display().to_string()),
            Err(Lookup::Unreadable(sentence)) => unreadable.push(sentence),
        }
    }
    Err(describe_not_found(wanted, &searched, &unreadable))
}

/// What to say when the name was in none of the directories.
///
/// 「查了哪些地方」 is spelled out rather than summarised. The defect this
/// replaced reported the *current* directory as though it were the only place
/// known, so a person whose file was in their previous folder was told their
/// file might have been deleted — the list is what makes the sentence checkable
/// against what they remember doing.
///
/// The two lists are separate and never merged: a directory that was read and
/// does not hold the name is evidence about the file, and a directory nobody
/// could look in is the absence of evidence. Only the first is a place the name
/// is known not to be.
fn describe_not_found(wanted: &str, searched: &[String], unreadable: &[String]) -> String {
    let mut parts = vec![format!("{wanted} 不在本机已知的保存位置里")];
    if !searched.is_empty() {
        parts.push(format!("已经查过：{}", searched.join("、")))
    }
    if !unreadable.is_empty() {
        parts.push(format!(
            "另有 {} 个位置没能查（那里的文件没看过）：{}",
            unreadable.len(),
            unreadable.join("；")
        ))
    }
    parts.push("可能已被移动或删除；如果它被搬到别处，可以在下载列表里重新下载".to_string());
    parts.join("。")
}

/// Pick the download save location in the system dialog and store the choice.
///
/// The dialog is opened where the operator's current choice is, so a second look
/// starts from the folder they last picked rather than from home. A settings file
/// that cannot be read is **not** reported here: the picker's job is to produce a
/// choice, the dialog still opens, and the store below reports the same file's
/// problems on the way out (`settings::save` reads before it writes).
///
/// The dialog call and the store are the two boundaries; everything between them
/// is [`chosen_path`], which is tested.
#[tauri::command]
pub async fn local_pick_save_directory(
    app: tauri::AppHandle,
) -> Result<Option<SettingsView>, String> {
    let mut dialog = app.dialog().file().set_title("选择下载保存位置");
    if let Some(current) = settings::chosen_save_dir().ok().flatten() {
        dialog = dialog.set_directory(current);
    }
    // Blocking, and `#[tauri::command] async` is what makes that correct: this
    // runs on the runtime's worker, not on the main thread the dialog has to
    // reach (`blocking_pick_folder`'s own doc says so). The plugin dispatches
    // `NSOpenPanel` to the main thread itself, which is why it is the plugin
    // rather than `rfd`'s synchronous API.
    let chosen = dialog.blocking_pick_folder();
    let Some(directory) = chosen_path(chosen)? else {
        return Ok(None);
    };
    // Through `settings::set_save_dir`, which is the **same** writer
    // `local_settings_set` uses, so the picker's value meets the same
    // `check_save_dir` — a second path into the settings file would be a second
    // chance for a directory the tasks cannot use to be stored.
    settings::set_save_dir(Some(&directory.display().to_string())).map(Some)
}

/// Push the stored save location to the Local Agent now.
///
/// Its own command rather than part of `local_settings_set`, because the choice
/// must be storable while the Agent is down: a push folded into the write would
/// report 「保存失败」for an Agent that is not running, which is the wrong
/// sentence and the wrong state.
#[tauri::command]
pub async fn local_push_save_directory(
    client: State<'_, LocalAgentClient>,
) -> Result<SaveDirectoryFacts, String> {
    push_chosen(&client, settings::chosen_save_dir()).await
}

/// Open one of this machine's downloaded files.
///
/// Takes a **name** from the page and looks it up in every save directory this
/// machine has used — `AGENT-INDEX.md`'s rule for the read and cleanup commands,
/// and the reason this needs no path parameter at all. Desktop-only by
/// construction: the Local Agent never composites and never opens anything, so
/// this is the app that has a file manager to reach.
///
/// **Every** directory, not the one in use: a file downloaded last week, before
/// the operator moved their save location, is still in the older folder, and
/// looking only where new files go would report it as gone. The choice of *where
/// new things go* and the question *where is this thing* are different questions
/// and only the second one has several answers.
///
/// The opener is the boundary, as in `commands::reveal`: whether the file then
/// appears in front of the person is not something a test can assert, and a test
/// that spawned it would open whatever the suite's machine has registered for
/// `.mp4`. Everything up to the spawn is [`find_in_known`], which is tested.
#[tauri::command]
pub fn local_open_saved_file(name: String) -> Result<String, String> {
    let found = find_in_known(&settings::known_save_dirs()?, &name)?;
    open::that(&found).map_err(|error| format!("无法打开 {}：{error}", found.display()))?;
    Ok(found.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::http::LocalAgentClient;
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;
    // The `Url` inside `FilePath::Url`, reached through `tauri`'s re-export so
    // this test does not have to name the `url` crate as a dependency of its own.
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use tauri::Url;

    /// The folder the tests hand over, and the one that must never come back in
    /// a message. Absolute, because the Agent refuses anything else, and under
    /// `/Volumes` so it reads as a path on this machine rather than as a fixture.
    const FOLDER: &str = "/Volumes/Movies/WTMedia";
    const CREDENTIAL: &str = "node-credential-77c1-secret";

    fn client_to(port: u16) -> LocalAgentClient {
        let text = PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}"));
        let config =
            load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config");
        LocalAgentClient::new(&config, RuntimeToken::generate())
    }

    /// A listener answering **every** request on it, recording all of them.
    ///
    /// Accepts until the client stops coming, rather than once, because a start
    /// offers two things and an arm that only ever answered one request could not
    /// tell 「the second was not sent」from 「the second was sent and nobody was
    /// listening」— which is the difference between the two pushes being
    /// independent and one of them being skipped.
    ///
    /// `answer` maps a status line to the body for it, so a test can refuse one
    /// route and accept the other.
    fn serving(
        answer: impl Fn(&str) -> (String, String) + Send + 'static,
    ) -> (u16, Arc<Mutex<Vec<String>>>) {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = server.local_addr().expect("the bound address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        std::thread::spawn(move || {
            for incoming in server.incoming() {
                let Ok(mut socket) = incoming else { break };
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                let mut complete = false;
                while let Ok(read) = socket.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            complete = true;
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&request).to_string();
                let path = text
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                sink.lock().expect("the recorder").push(text);
                if !complete {
                    break;
                }
                let (status, body) = answer(&path);
                let response = format!(
                    "HTTP/1.1 {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                if socket.write_all(response.as_bytes()).is_err() {
                    break;
                }
            }
        });
        (port, seen)
    }

    /// The Agent's answer to a stored directory.
    fn stored(dir: &str) -> String {
        format!(r#"{{"data":{{"save_dir":"{dir}","writable":true,"free_bytes":4096}}}}"#)
    }

    /// What the stub received, joined.
    fn received(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock().expect("the recorder").clone()
    }

    /// How many requests arrived for `route`.
    fn arrivals(seen: &Arc<Mutex<Vec<String>>>, route: &str) -> usize {
        received(seen)
            .iter()
            .filter(|request| {
                request
                    .split_whitespace()
                    .nth(1)
                    .is_some_and(|path| path == route)
            })
            .count()
    }

    /// A port nothing is listening on: bound, then released.
    ///
    /// The negative arms use this rather than a stub that records nothing, and
    /// the difference matters: an attempted push against a closed port is an
    /// `Err` from `send()`, so 「什么也没推」 is only proved by a call that would
    /// have failed had it tried.
    fn silent_port() -> u16 {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        server.local_addr().expect("the bound address").port()
    }

    fn binding() -> RuntimeBinding {
        RuntimeBinding {
            node_id: "node-a".into(),
            node_credential: CREDENTIAL.into(),
        }
    }

    /// The body is one key, spelled the Agent's way.
    #[test]
    fn the_save_directory_body_is_the_agents_one_key() {
        let body = save_directory_body(FOLDER);

        let keys: Vec<&str> = body
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["save_dir"]);
        assert_eq!(body["save_dir"], FOLDER);
    }

    /// The folder reaches the route, and the Agent's own facts come back.
    #[test]
    fn a_pushed_directory_answers_with_the_agents_facts() {
        let (port, seen) = serving(|_| ("200 OK".into(), stored(FOLDER)));

        let returned = tauri::async_runtime::block_on(push(&client_to(port), FOLDER));

        let facts = returned.expect("the Agent stored it");
        assert_eq!(
            facts,
            SaveDirectoryFacts {
                save_dir: Some(FOLDER.to_string()),
                writable: true,
                free_bytes: 4096,
            }
        );
        let requests = received(&seen);
        assert_eq!(arrivals(&seen, "/api/v1/save-directory"), 1);
        assert!(
            requests[0].contains(FOLDER),
            "the folder has to travel: {}",
            requests[0]
        );
    }

    /// A refusal is named by the Agent's code, and **without the path**.
    ///
    /// The second half is the one a future edit could undo without noticing: the
    /// message this returns is what `commands::agent::offer_failed` records, and
    /// CHG-061 §4 keeps this machine's absolute paths out of records. Asserting
    /// on the whole message rather than on a fragment, so a message that grows a
    /// `format!("…{save_dir}…")` fails here rather than in a log file.
    #[test]
    fn a_refused_directory_is_named_by_its_code_and_carries_no_path() {
        let (port, _seen) = serving(|_| {
            (
                "400 Bad Request".into(),
                r#"{"error":{"code":"save_directory_invalid"}}"#.into(),
            )
        });

        let returned = tauri::async_runtime::block_on(push(&client_to(port), FOLDER));

        let message = returned.expect_err("a 400 is a failure");
        assert!(
            message.contains("save_directory_invalid"),
            "the code is what a caller can act on: {message}"
        );
        assert!(
            !message.contains(FOLDER) && !message.contains("Movies"),
            "a local path reached a message that is recorded: {message}"
        );
    }

    /// A body that is not a refusal is the status alone, and still no path.
    #[test]
    fn an_unrecognised_refusal_body_reports_the_status_alone() {
        let (port, _seen) = serving(|_| ("503 Service Unavailable".into(), "oops".into()));

        let message = tauri::async_runtime::block_on(push(&client_to(port), FOLDER))
            .expect_err("a 503 is a failure");

        assert!(message.contains("503"), "{message}");
        assert!(!message.contains('（'), "nothing is invented: {message}");
        assert!(!message.contains("Movies"), "no path: {message}");
    }

    /// 「没有选择」puts nothing on the wire.
    ///
    /// Asserted against a **closed** port: had a request been attempted it would
    /// have come back as a connection error and this arm would return
    /// `Some(reason)` instead of `None`. A stub that merely recorded nothing
    /// would pass for the wrong reason here, because `offer_directory` returns
    /// `None` for a successful push too.
    #[test]
    fn an_unchosen_directory_pushes_nothing() {
        let port = silent_port();

        let returned = tauri::async_runtime::block_on(offer_directory(&client_to(port), None));

        assert_eq!(returned, None, "nothing to say, so nothing is said");
    }

    /// An unbound Desktop has no credential to re-state.
    #[test]
    fn an_unbound_desktop_offers_no_credential() {
        let port = silent_port();

        let returned = tauri::async_runtime::block_on(offer_credential(&client_to(port), None));

        assert_eq!(returned, None);
    }

    /// Both things a start says arrive, over their two routes.
    #[test]
    fn a_start_offers_the_credential_and_the_folder_together() {
        let (port, seen) = serving(|path| {
            match path {
            "/api/v1/bind" => (
                "200 OK".into(),
                r#"{"node_id":"node-a","session_token":"s","status":"active","has_node_credential":true}"#
                    .into(),
            ),
            _ => ("200 OK".into(), stored(FOLDER)),
        }
        });

        let failures = tauri::async_runtime::block_on(offer_stored_choices(
            &client_to(port),
            Some(&binding()),
            Ok(Some(FOLDER.to_string())),
        ));

        assert_eq!(failures, Vec::<String>::new(), "nothing to report");
        let requests = received(&seen);
        assert_eq!(arrivals(&seen, "/api/v1/bind"), 1);
        assert_eq!(arrivals(&seen, "/api/v1/save-directory"), 1);
        let bind = requests
            .iter()
            .find(|request| request.contains("/api/v1/bind"))
            .expect("the bind request");
        assert!(
            bind.contains(CREDENTIAL) && bind.contains("node-a"),
            "the credential and the node it belongs to: {bind}"
        );
        let directory = requests
            .iter()
            .find(|request| request.contains("/api/v1/save-directory"))
            .expect("the directory request");
        assert!(directory.contains(FOLDER), "{directory}");
    }

    /// One push failing does not stop the other.
    ///
    /// The property `offer_stored_choices` is a separate function for, and one
    /// `?` would undo: the Agent answers a 503 to the credential route and a 200
    /// to the directory route, and **both** requests arrive. Without this arm a
    /// composition that propagated the first failure would be indistinguishable
    /// from a correct one in every other test here, and the folder would go
    /// unset for a reason that has nothing to do with the folder.
    #[test]
    fn a_refused_credential_does_not_stop_the_folder_from_being_offered() {
        let (port, seen) = serving(|path| {
            if path == "/api/v1/bind" {
                (
                    "503 Service Unavailable".into(),
                    r#"{"error":{"code":"node_credential_unavailable"}}"#.into(),
                )
            } else {
                ("200 OK".into(), stored(FOLDER))
            }
        });

        let failures = tauri::async_runtime::block_on(offer_stored_choices(
            &client_to(port),
            Some(&binding()),
            Ok(Some(FOLDER.to_string())),
        ));

        assert_eq!(
            failures.len(),
            1,
            "one refusal, reported once: {failures:?}"
        );
        assert!(
            failures[0].contains("node_credential_unavailable"),
            "{failures:?}"
        );
        assert_eq!(
            arrivals(&seen, "/api/v1/save-directory"),
            1,
            "the folder was still offered"
        );
    }

    /// A settings file that cannot be read is reported, and stops nothing.
    ///
    /// The `Err` arm of `chosen`: a corrupt or foreign-schema settings file is a
    /// reason the folder will not be set, and it is a different sentence from
    /// 「没有选择」 — so it is recorded rather than read as 「nothing chosen」. The
    /// credential push still happens, because the two have nothing to do with
    /// each other.
    #[test]
    fn a_settings_file_that_cannot_be_read_is_reported_and_offers_nothing() {
        let (port, seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"node_id":"node-a","session_token":"s","status":"active","has_node_credential":true}"#
                    .into(),
            )
        });

        let failures = tauri::async_runtime::block_on(offer_stored_choices(
            &client_to(port),
            Some(&binding()),
            Err("读取设置失败：schema_version 不认识".into()),
        ));

        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("读取设置失败"), "{failures:?}");
        assert_eq!(
            arrivals(&seen, "/api/v1/save-directory"),
            0,
            "a value that could not be read must not be guessed at"
        );
        assert_eq!(
            arrivals(&seen, "/api/v1/bind"),
            1,
            "and the credential still goes"
        );
    }

    /// A start whose Agent cannot be reached reports both, and does not panic.
    ///
    /// The failure mode of a re-push against an Agent that died between the
    /// readiness gate and this call — narrow, and the reason the composition
    /// collects reasons rather than returning an error into the start. Two
    /// failures, because the two attempts are independent, and no path in
    /// either — these strings are what `offer_failed` records.
    #[test]
    fn an_unreachable_agent_is_reported_rather_than_propagated() {
        let port = silent_port();

        let failures = tauri::async_runtime::block_on(offer_stored_choices(
            &client_to(port),
            Some(&binding()),
            Ok(Some(FOLDER.to_string())),
        ));

        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(
            failures[0].contains("写入Local Agent节点失败"),
            "{failures:?}"
        );
        assert!(
            failures[1].contains("写入Local Agent保存目录失败"),
            "{failures:?}"
        );
        for reason in &failures {
            assert!(
                !reason.contains("Movies") && !reason.contains(CREDENTIAL),
                "no path and no secret on the way to a record: {reason}"
            );
        }
    }

    // ---- the picker's answer ----

    /// A cancelled dialog is `None`, not a failure.
    ///
    /// The operator closing the picker is a decision the page has to be able to
    /// tell apart from 「选了但存不上」: the first is nothing to report, and a
    /// command that turned it into an `Err` would show a person their own click
    /// back as a fault.
    #[test]
    fn a_cancelled_dialog_is_not_a_failure() {
        let got = chosen_path(None).expect("cancelling is not an error");

        assert_eq!(got, None);
    }

    /// A picked path comes back exactly as the picker gave it.
    #[test]
    fn a_picked_path_comes_back_unchanged() {
        let path = PathBuf::from(FOLDER);

        let got = chosen_path(Some(FilePath::Path(path.clone()))).expect("a path is a path");

        assert_eq!(got, Some(path));
    }

    /// The `file://` form converts; a URL that names no file is refused.
    ///
    /// The pair is the point: the same enum variant, two outcomes, so the refusal
    /// is provably about the URL not naming a file rather than about it being a
    /// URL at all. `content://` is what an Android build's picker answers with;
    /// the directory this app stores has to be one this machine can write to.
    #[test]
    fn a_file_url_converts_and_another_scheme_is_refused() {
        let file = Url::parse("file:///tmp/Movies").expect("a file url");
        let converted = chosen_path(Some(FilePath::Url(file))).expect("file:// converts");

        let mut refusals = Vec::new();
        for spelling in ["https://example.invalid/share", "content://media/external"] {
            let url = Url::parse(spelling).expect("a url");
            let error = chosen_path(Some(FilePath::Url(url))).expect_err(spelling);
            assert!(error.contains("不是本机目录"), "{spelling}: {error}");
            // The refusal must not quote what it refused: a share URL can carry a
            // token, and this message is what a page shows and a log records.
            assert!(!error.contains(spelling), "{spelling}: {error}");
            refusals.push(error);
        }

        assert_eq!(converted, Some(PathBuf::from("/tmp/Movies")));
        assert_eq!(refusals.len(), 2, "both schemes are in the loop above");
    }

    // ---- the push's three answers ----

    /// 「还没有选择」 is a sentence, and **nothing is pushed**.
    ///
    /// The port is a closed one, so had the push been attempted this would have
    /// come back as a connection failure instead — which is what makes the exact
    /// message below a reading rather than an assumption about a `None` arm.
    #[test]
    fn an_unchosen_choice_is_reported_rather_than_pushed() {
        let port = silent_port();

        let returned = tauri::async_runtime::block_on(push_chosen(&client_to(port), Ok(None)));

        assert_eq!(
            returned.expect_err("there is nothing to push"),
            "还没有选择下载保存位置"
        );
    }

    /// A settings file that cannot be read is reported as itself.
    ///
    /// Distinct from 「没有选择」 on purpose: one is a person who has not decided,
    /// the other is a file this build cannot understand, and the two want
    /// different things done about them. Neither is a push.
    #[test]
    fn a_settings_file_that_cannot_be_read_is_not_pushed() {
        let port = silent_port();
        let broken = "读取设置失败：settings.toml 的 schema 不认识".to_string();

        let returned =
            tauri::async_runtime::block_on(push_chosen(&client_to(port), Err(broken.clone())));

        assert_eq!(returned.expect_err("must fail"), broken);
    }

    /// A stored choice reaches the Agent, and the Agent's facts come back.
    ///
    /// The composition's happy arm: what `settings::chosen_save_dir` returned is
    /// what travels, and the answer is the Agent's rather than an echo.
    #[test]
    fn a_stored_choice_is_pushed_and_answered_by_the_agent() {
        let (port, seen) = serving(|_| ("200 OK".into(), stored(FOLDER)));

        let returned =
            tauri::async_runtime::block_on(push_chosen(&client_to(port), Ok(Some(FOLDER.into()))));

        let facts = returned.expect("the Agent stored it");
        assert_eq!(facts.save_dir, Some(FOLDER.to_string()));
        assert_eq!(arrivals(&seen, "/api/v1/save-directory"), 1);
        assert!(
            received(&seen)[0].contains(FOLDER),
            "the folder has to travel"
        );
    }

    // ---- opening a downloaded file ----

    /// A scratch directory of this test's own, removed by the caller.
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-downloads-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).expect("scratch dir");
        path
    }

    /// A name that is in the directory comes back as that entry's path.
    #[test]
    fn a_name_in_the_save_directory_comes_back_as_its_path() {
        let root = scratch("found");
        let wanted = root.join("标题-素材.mp4");
        std::fs::write(&wanted, b"x").expect("a downloaded file");

        let found = find_saved_file(&root, "标题-素材.mp4");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.expect("it is there"), wanted);
    }

    /// Every name a listing gives is looked up **as it is spelled**.
    ///
    /// The property the page depends on: the drawer shows names that came from the
    /// task records, and each of them has to be openable. Driven from the listing
    /// itself rather than from this test's literals, so the fixture and the input
    /// cannot be two different sets of names — and the awkward shapes are in it
    /// deliberately, because a lookup that trimmed its input would pass a fixture
    /// of tidy names and leave 「列表里有、点开说没有」 as the defect.
    #[test]
    fn every_name_in_a_listing_is_looked_up_as_it_is_spelled() {
        let root = scratch("listing");
        for name in [
            "plain.mp4",
            "with space.mp4",
            " leading-space.mp4",
            "trailing-space.mp4 ",
            "a.b.c.mp4",
            ".hidden.mp4",
            "emoji-\u{1f3ac}.mp4",
        ] {
            std::fs::write(root.join(name), b"x").unwrap_or_else(|error| panic!("{name}: {error}"));
        }
        let listed: Vec<(String, PathBuf)> = std::fs::read_dir(&root)
            .expect("the listing")
            .map(|entry| {
                let entry = entry.expect("an entry");
                (
                    entry.file_name().to_str().expect("a name").to_string(),
                    entry.path(),
                )
            })
            .collect();

        let mut looked_up = Vec::new();
        for (name, path) in &listed {
            let found = find_saved_file(&root, name)
                .unwrap_or_else(|error| panic!("the listing offered {name:?}: {error}"));
            looked_up.push((name.clone(), found, path.clone()));
        }

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(looked_up.len(), 7, "the fixture itself: {looked_up:?}");
        for (name, found, path) in looked_up {
            assert_eq!(found, path, "「{name}」 resolved to the wrong entry");
        }
    }

    /// A name the directory does not hold is reported as missing, not as a path.
    ///
    /// Both halves matter: the sentence says which directory and which name — a
    /// person has to be able to see whether it is the folder they meant — and it
    /// is a different sentence from the path refusal below, so a page (or a log)
    /// can tell 「名字不对」 from 「这种输入根本不收」.
    #[test]
    fn a_name_that_is_not_in_the_directory_is_reported_as_missing() {
        let root = scratch("missing");
        std::fs::write(root.join("kept.mp4"), b"x").expect("a file");

        let error = find_saved_file(&root, "gone.mp4").expect_err("must not be found");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("里没有 gone.mp4"), "{error}");
        assert!(error.contains(&root.display().to_string()), "{error}");
        assert!(!error.contains("不是一个文件名"), "{error}");
    }

    /// A path is not a name, in every spelling that could name one — and the two
    /// spellings that **are** one component are looked up rather than refused.
    ///
    /// Enumerated rather than sampled, and split along the rule's own boundary,
    /// because the check is 「a name is one path component」 and not 「the text
    /// looks tidy」: on this platform `\` and a space are ordinary characters, so
    /// `C:\Windows` and `"   "` are names, and a test that claimed otherwise would
    /// pass for the wrong reason. What matters for both groups is that neither
    /// reaches anything outside the save directory.
    #[test]
    fn a_path_is_not_a_name() {
        use std::os::unix::fs::symlink;

        let parent = scratch("shapes");
        let save = parent.join("save");
        std::fs::create_dir_all(&save).expect("the save directory");
        // Outside the save directory, so a lookup that joined the name onto the
        // directory would find something and the refusal would be a lie.
        let outside = parent.join("secret.mp4");
        std::fs::write(&outside, b"x").expect("a file the page must not reach");
        symlink(&outside, save.join("linked.mp4")).expect("a link to it");
        let outside_name = outside.display().to_string();

        let not_a_name = [
            "",
            ".",
            "..",
            "/",
            "/etc/passwd",
            "../secret.mp4",
            "sub/secret.mp4",
            "~/secret.mp4",
            "file:///etc/passwd",
            outside_name.as_str(),
        ];
        let one_component = ["   ", "C:\\Windows", "..hidden.mp4"];

        let mut wrong = Vec::new();
        for name in not_a_name {
            match find_saved_file(&save, name) {
                Ok(found) => wrong.push(format!("{name:?} was accepted as {}", found.display())),
                Err(error) if !error.contains("不是一个文件名") => {
                    wrong.push(format!("{name:?} got the wrong sentence: {error}"))
                }
                Err(_) => {}
            }
        }
        for name in one_component {
            match find_saved_file(&save, name) {
                Ok(found) => wrong.push(format!("{name:?} was accepted as {}", found.display())),
                Err(error) if !error.contains("里没有") => {
                    wrong.push(format!("{name:?} got the wrong sentence: {error}"))
                }
                Err(_) => {}
            }
        }
        // The link is in the directory and is still not a download: a name that
        // is one component does not get to reach whatever it points at.
        let linked = find_saved_file(&save, "linked.mp4");

        std::fs::remove_dir_all(&parent).ok();

        assert!(
            wrong.is_empty(),
            "{} shapes, {} wrong: {wrong:?}",
            not_a_name.len() + one_component.len(),
            wrong.len()
        );
        assert_eq!(not_a_name.len(), 10, "the refused group, as written above");
        assert_eq!(
            one_component.len(),
            3,
            "the looked-up group, as written above"
        );
        let linked = linked.expect_err("a symlink is not a downloaded file");
        assert!(
            !linked.contains("secret"),
            "the refusal must not say where the link pointed: {linked}"
        );
    }

    /// A directory that shares the name is not a download either.
    #[test]
    fn a_directory_that_shares_the_name_is_not_a_download() {
        let root = scratch("directory");
        std::fs::create_dir_all(root.join("movie.mp4")).expect("a directory named like one");

        let error = find_saved_file(&root, "movie.mp4").expect_err("not a file");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("里没有"), "{error}");
    }

    /// A save directory that is not a directory says so, and says what to do.
    #[test]
    fn a_save_directory_that_is_not_a_directory_is_reported() {
        let root = scratch("stale");
        let file = root.join("was-a-folder");
        std::fs::write(&file, b"x").expect("a file where the directory was");

        let error = find_saved_file(&file, "movie.mp4").expect_err("not a directory");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("不是一个目录"), "{error}");
        assert!(
            error.contains("重新选"),
            "it has to say what to do: {error}"
        );
    }

    /// A save directory that cannot be listed at all is reported as that.
    ///
    /// The arm an unmounted volume or a changed permission produces, and the one
    /// that must not be mistaken for 「还没有下载」: the message names the
    /// directory, and the directory is still there afterwards.
    #[test]
    fn a_save_directory_that_cannot_be_listed_is_reported() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("locked");
        let locked = root.join("locked");
        std::fs::create_dir_all(&locked).expect("a directory");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("lock it");

        let error = find_saved_file(&locked, "movie.mp4").expect_err("cannot be listed");

        // Unlocked before the cleanup: a directory with no permission bits cannot
        // be removed, and a suite that left one behind would fail the next run.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("unlock");
        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("无法读取保存位置"), "{error}");
        assert!(error.contains(&locked.display().to_string()), "{error}");
    }

    /// A file in the **older** directory is found there.
    ///
    /// This is the defect 「已下载的改完保存路径后就找不到文件了」 as an assertion,
    /// and the older directory is deliberately the *second* one: a lookup that
    /// only ever tried the directory in use would pass a test whose file sits in
    /// the current folder, and would fail this one.
    #[test]
    fn a_file_in_an_older_save_directory_is_found_there() {
        let root = scratch("history-hit");
        let current = root.join("now");
        let older = root.join("before");
        std::fs::create_dir_all(&current).expect("the directory in use");
        std::fs::create_dir_all(&older).expect("the directory it used to be");
        let wanted = older.join("标题-素材.mp4");
        std::fs::write(&wanted, b"x").expect("a file downloaded before the move");
        let dirs = vec![current, older];

        let found = find_in_known(&dirs, "标题-素材.mp4");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.expect("it is still there"), wanted);
    }

    /// The directory in use wins when both hold the name.
    ///
    /// Order is not decoration: the list is newest-first, and a file re-downloaded
    /// after a move would otherwise be shadowed by the copy it replaced.
    #[test]
    fn the_newest_directory_wins_when_the_name_is_in_two_of_them() {
        let root = scratch("newest-wins");
        let current = root.join("now");
        let older = root.join("before");
        std::fs::create_dir_all(&current).expect("the directory in use");
        std::fs::create_dir_all(&older).expect("the directory it used to be");
        std::fs::write(current.join("same.mp4"), b"new").expect("the copy that is current");
        std::fs::write(older.join("same.mp4"), b"old").expect("the copy it replaced");

        let found = find_in_known(&[current.clone(), older], "same.mp4");

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.expect("found"), current.join("same.mp4"));
    }

    /// A name in none of them names every directory that was looked in.
    ///
    /// 「查了哪些地方」 has to be in the sentence: the defect this replaced named
    /// the current directory as though it were the only one known, so a person
    /// whose file was in their previous folder was told it might have been
    /// deleted. The two directories here are both **readable and empty**, so the
    /// message may not hedge — nothing was unreadable.
    #[test]
    fn a_name_in_no_known_directory_names_every_directory_searched() {
        let root = scratch("history-miss");
        let current = root.join("now");
        let older = root.join("before");
        std::fs::create_dir_all(&current).expect("the directory in use");
        std::fs::create_dir_all(&older).expect("the directory it used to be");

        let error = find_in_known(&[current.clone(), older.clone()], "gone.mp4")
            .expect_err("nowhere to be found");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("已经查过"), "{error}");
        assert!(error.contains(&current.display().to_string()), "{error}");
        assert!(error.contains(&older.display().to_string()), "{error}");
        assert!(!error.contains("没能查"), "nothing was unreadable: {error}");
        assert!(
            error.contains("重新下载"),
            "and it has to say what the way out is: {error}"
        );
    }

    /// A directory nobody can read is reported as unread, not as an absence.
    ///
    /// The distinction the whole message turns on: 「已经查过 X」 is evidence that
    /// the file is not there, and 「X 没能查」 is the absence of evidence. Merging
    /// them is how a file sitting on an unplugged volume gets reported as
    /// deleted, and how somebody re-downloads 230 MB they still have.
    #[test]
    fn a_directory_that_cannot_be_read_is_not_reported_as_an_absence() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("history-unreadable");
        let current = root.join("now");
        let locked = root.join("locked");
        std::fs::create_dir_all(&current).expect("the directory in use");
        std::fs::create_dir_all(&locked).expect("a directory about to be unreadable");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("lock it");

        let error = find_in_known(&[current.clone(), locked.clone()], "gone.mp4")
            .expect_err("nowhere to be found");

        // Unlocked before the cleanup: a directory with no permission bits cannot
        // be removed, and a suite that left one behind would fail the next run.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("unlock");
        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("没能查"), "{error}");
        assert!(error.contains(&locked.display().to_string()), "{error}");
        assert!(
            error.contains(&current.display().to_string()),
            "the directory that *was* read is still named: {error}"
        );
    }

    /// A directory that is no longer a directory is one of the unread ones.
    ///
    /// The unmounted-volume shape, and the one a person reaches by deleting the
    /// folder: it is listed with its own sentence rather than swallowed.
    #[test]
    fn a_directory_that_is_no_longer_a_directory_is_named_as_unread() {
        let root = scratch("history-not-a-dir");
        let gone = root.join("was-a-folder");
        std::fs::write(&gone, b"x").expect("a file where the directory was");

        let error = find_in_known(&[gone.clone()], "movie.mp4").expect_err("nowhere to look");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("没能查"), "{error}");
        assert!(error.contains("不是一个目录"), "{error}");
    }

    /// Nothing ever chosen has nowhere to look, and says that.
    #[test]
    fn an_empty_search_space_says_nothing_has_been_chosen() {
        let error = find_in_known(&[], "movie.mp4").expect_err("nowhere to look");

        assert!(error.contains("还没有选择下载保存位置"), "{error}");
    }

    /// A path is refused before any directory is touched.
    ///
    /// The rule does not weaken because there are now several directories to
    /// search — `find_in_known` checks the name once, at the top, and the
    /// directories are never consulted for a name that is not one. Asserted with
    /// a directory list that **would** have found it, so a lookup that searched
    /// first and refused later cannot pass.
    #[test]
    fn a_path_shaped_name_is_refused_before_any_directory_is_searched() {
        let root = scratch("history-name");
        std::fs::write(root.join("movie.mp4"), b"x").expect("a file");

        let error = find_in_known(&[root.clone()], "../movie.mp4").expect_err("not a name");

        std::fs::remove_dir_all(&root).ok();

        assert!(error.contains("不是一个文件名"), "{error}");
        assert!(
            !error.contains(&root.display().to_string()),
            "the directories must not even be listed: {error}"
        );
    }

    /// Every command this module declares is registered, and no others are.
    ///
    /// A `#[tauri::command]` that nobody put in `generate_handler!` compiles,
    /// passes its own tests, and fails at runtime as 「command not found」 — the
    /// shape of failure the capability note above warns about. Nothing else in the
    /// tree compares the two lists: `localAgentService.test.js` asserts the
    /// argument objects of the commands the page calls, which is the other
    /// direction and only covers what the page already uses.
    ///
    /// Read as source text rather than through any macro, because that is what
    /// 「registered」 means here. The reader takes only lines that are exactly the
    /// attribute and only entries that are not comments, which is the difference
    /// between this and a `grep` that counts a doc comment mentioning the
    /// attribute — that mistake was made once already in this CHG.
    #[test]
    fn every_command_in_this_module_is_registered() {
        fn declared(source: &str) -> Vec<String> {
            let lines: Vec<&str> = source.lines().collect();
            let mut names = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if line.trim() != "#[tauri::command]" {
                    continue;
                }
                let signature = lines[index..]
                    .iter()
                    .find(|line| line.contains("fn "))
                    .unwrap_or_else(|| {
                        panic!("no signature after the attribute at line {}", index + 1)
                    });
                let after = signature.split("fn ").nth(1).expect("a name");
                names.push(
                    after
                        .split('(')
                        .next()
                        .expect("an open paren")
                        .trim()
                        .to_string(),
                );
            }
            names
        }

        fn registered(main: &str) -> Vec<String> {
            let start = main.find("generate_handler![").expect("the handler list");
            let rest = &main[start..];
            let end = rest.find("])").expect("the end of the handler list");
            let mut names = Vec::new();
            for line in rest[..end].lines() {
                let entry = line.trim().trim_end_matches(',');
                if entry.starts_with("//") {
                    continue;
                }
                if let Some(rest) = entry.strip_prefix("commands::downloads::") {
                    names.push(rest.to_string());
                }
            }
            names
        }

        let mut declared = declared(include_str!("downloads.rs"));
        let mut registered = registered(include_str!("../main.rs"));
        declared.sort();
        registered.sort();

        assert_eq!(
            declared, registered,
            "the module's commands and the app's handler list are not the same set"
        );
        assert_eq!(
            declared.len(),
            3,
            "the denominator, measured here: {declared:?}"
        );
    }
}
