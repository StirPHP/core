//! Running PHPStan, and being sure it's gone afterwards. It starts a process per CPU core for its
//! workers, so it gets a process group of its own: that's what's killed, not only the process we
//! started.

use std::future::Future;
use std::process::{ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::find::Invocation;
use crate::report::{Report, is_memory, parse};

/// More output than this isn't a report of a project's errors. Laravel's whole framework at the
/// highest level is far from it.
const MAX_STDOUT: usize = 256 * 1024 * 1024;
/// What's kept of the end of stderr, to say what went wrong.
const MAX_STDERR: usize = 64 * 1024;
/// How long output may keep coming after everything we know of is dead: something that left the
/// process group may hold on to the pipe forever.
const DRAIN_WAIT: Duration = Duration::from_secs(2);

/// Why a run has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Stopped by whoever started it.
    Cancelled,
    TimedOut(Duration),
    /// It couldn't be started at all.
    NotStarted(String),
    OutOfMemory(String),
    TooMuchOutput,
    /// It ended without a report: a broken configuration, a PHP that's too old, a crash.
    NoReport {
        code: Option<i32>,
        /// What it said, without what it says on every run.
        output: String,
    },
}

impl Failure {
    /// The same kind of failure is told about once.
    pub fn kind(&self) -> &'static str {
        match self {
            Failure::Cancelled => "cancelled",
            Failure::TimedOut(_) => "timeout",
            Failure::NotStarted(_) => "start",
            Failure::OutOfMemory(_) => "memory",
            Failure::TooMuchOutput => "output",
            Failure::NoReport { .. } => "report",
        }
    }

    /// One thing to tell a person.
    pub fn describe(&self, invocation: &Invocation) -> String {
        let command = invocation.shown();
        match self {
            Failure::Cancelled => format!("`{command}` was stopped."),
            Failure::TimedOut(after) => format!(
                "`{command}` was stopped after {} seconds. A longer `timeout` can be set in `parameters.stirphp.phpstan` or the editor's settings.",
                after.as_secs()
            ),
            Failure::NotStarted(why) => format!("`{command}` couldn't start: {why}."),
            Failure::OutOfMemory(_) => format!(
                "`{command}` ran out of memory. A higher `memoryLimit` can be set in `parameters.stirphp.phpstan` or the editor's settings."
            ),
            Failure::TooMuchOutput => format!("`{command}` wrote too much to be a report."),
            Failure::NoReport { code, output } => {
                let code = code.map_or_else(|| "a signal".to_owned(), |c| format!("code {c}"));
                let said = summary(output);
                if said.is_empty() {
                    format!("`{command}` ended with {code} and no report.")
                } else {
                    format!("`{command}` ended with {code} and no report: {said}")
                }
            }
        }
    }

    /// All of what it said, for a log.
    pub fn output(&self) -> Option<&str> {
        match self {
            Failure::OutOfMemory(output) | Failure::NoReport { output, .. } => Some(output),
            _ => None,
        }
    }
}

/// The beginning of what's said, on one line. PHP and Symfony's console wrap and pad their errors.
fn summary(output: &str) -> String {
    const MAX: usize = 400;
    let boilerplate = |line: &str| {
        line.is_empty()
            || line.starts_with("Note: ")
            || line.starts_with("analyse [")
            || (line.starts_with("In ") && line.ends_with(':'))
    };
    let lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !boilerplate(line));
    let mut said = lines.take(4).collect::<Vec<_>>().join(" ");
    if said.len() > MAX {
        let mut end = MAX;
        while !said.is_char_boundary(end) {
            end -= 1;
        }
        said.truncate(end);
        said.push('…');
    }
    said
}

/// The process groups of runs under way. A signal doesn't leave time to go through the tasks they
/// belong to.
static RUNNING: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Kills all of a run on the way out, however that's reached.
struct Group(u32);

impl Group {
    fn new(id: u32) -> Group {
        RUNNING.lock().unwrap().push(id);
        Group(id)
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        RUNNING.lock().unwrap().retain(|id| *id != self.0);
        kill_group(self.0);
    }
}

#[cfg(unix)]
fn kill_group(id: u32) {
    use rustix::process::{Pid, Signal, kill_process_group};
    if let Some(pid) = i32::try_from(id).ok().and_then(Pid::from_raw) {
        // gone already, when it ended by itself
        let _ = kill_process_group(pid, Signal::KILL);
    }
}

#[cfg(not(unix))]
fn kill_group(id: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &id.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Kills every run under way, for when the process is on its way out.
pub fn kill_all() {
    let ids = std::mem::take(&mut *RUNNING.lock().unwrap());
    for id in ids {
        kill_group(id);
    }
}

/// Resolves when this process is told to stop.
pub async fn signalled() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut hangup), Ok(mut interrupt)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
            signal(SignalKind::interrupt()),
        ) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = hangup.recv() => {}
            _ = interrupt.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Runs PHPStan until there's a report, or until `cancel` resolves. Everything it started is gone
/// when this returns, or if this is dropped.
pub async fn run(
    invocation: &Invocation,
    cancel: impl Future<Output = ()>,
) -> Result<Report, Failure> {
    let mut command = tokio::process::Command::new(&invocation.program);
    command
        .args(invocation.argv())
        .current_dir(&invocation.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut tries = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            // being written, say by Composer, or by another process forking just then
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 5 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(Failure::NotStarted(not_started(invocation, &error))),
        }
    };
    let Some(id) = child.id() else {
        return Err(Failure::NotStarted("it ended at once".to_owned()));
    };
    let group = Group::new(id);

    let stdout = child
        .stdout
        .take()
        .map(|out| tokio::spawn(read_up_to(out, MAX_STDOUT)));
    let stderr = child
        .stderr
        .take()
        .map(|err| tokio::spawn(read_tail(err, MAX_STDERR)));
    let status: ExitStatus = tokio::select! {
        status = child.wait() => status.map_err(|e| Failure::NotStarted(e.to_string()))?,
        () = tokio::time::sleep(invocation.timeout) => {
            drop(group);
            let _ = child.wait().await;
            return Err(Failure::TimedOut(invocation.timeout));
        }
        () = cancel => {
            drop(group);
            let _ = child.wait().await;
            return Err(Failure::Cancelled);
        }
    };
    // whatever it left behind goes too, which also ends the output
    drop(group);

    let (out, complete) = match stdout {
        Some(task) => finished(task).await.unwrap_or_default(),
        None => (Vec::new(), true),
    };
    let err = match stderr {
        Some(task) => finished(task).await.map(|(err, _)| err).unwrap_or_default(),
        None => Vec::new(),
    };
    if !complete {
        return Err(Failure::TooMuchOutput);
    }
    let out = String::from_utf8_lossy(&out);
    if let Some(report) = parse(&out, &invocation.cwd) {
        return Ok(report);
    }
    let err = String::from_utf8_lossy(&err);
    // the configuration's errors go to stdout or stderr, depending
    let output = format!("{err}\n{out}").trim().to_owned();
    if is_memory(&output) {
        Err(Failure::OutOfMemory(output))
    } else {
        Err(Failure::NoReport {
            code: status.code(),
            output,
        })
    }
}

/// Why `program` wouldn't start, in terms of what to do about it.
fn not_started(invocation: &Invocation, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        let program = invocation.program.display();
        // it could also be the folder, but `locate` looked at that
        return format!("there's no `{program}` to run");
    }
    error.to_string()
}

/// The result of a reading task, once it's had the time it gets.
async fn finished(task: tokio::task::JoinHandle<(Vec<u8>, bool)>) -> Option<(Vec<u8>, bool)> {
    let abort = task.abort_handle();
    match tokio::time::timeout(DRAIN_WAIT, task).await {
        Ok(result) => result.ok(),
        Err(_) => {
            abort.abort();
            None
        }
    }
}

/// Up to `limit` bytes, and whether that's all there is. Past the limit it goes on reading, so that
/// the writer isn't stuck.
async fn read_up_to(mut from: impl AsyncRead + Unpin, limit: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut chunk = vec![0; 64 * 1024];
    let mut complete = true;
    while let Ok(n @ 1..) = from.read(&mut chunk).await {
        if out.len() + n > limit {
            complete = false;
        } else {
            out.extend_from_slice(&chunk[..n]);
        }
    }
    (out, complete)
}

/// The last `limit` bytes at most.
async fn read_tail(mut from: impl AsyncRead + Unpin, limit: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut chunk = vec![0; 16 * 1024];
    while let Ok(n @ 1..) = from.read(&mut chunk).await {
        out.extend_from_slice(&chunk[..n]);
        if out.len() > 2 * limit {
            out.drain(..out.len() - limit);
        }
    }
    (out, true)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    use super::*;

    /// A `phpstan` that's this shell script.
    fn fake(dir: &Path, script: &str, timeout: Duration) -> Invocation {
        let path = dir.join("phpstan");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Invocation {
            program: path,
            script: None,
            args: vec!["analyse".into()],
            cwd: dir.to_owned(),
            config_file: None,
            timeout,
        }
    }

    const LONG: Duration = Duration::from_secs(60);

    fn alive(pid: &str) -> bool {
        // a process that's been killed but not waited for is as good as gone
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
    }

    async fn wait_for_file(path: &PathBuf) -> String {
        for _ in 0..500 {
            if let Ok(text) = std::fs::read_to_string(path)
                && !text.trim().is_empty()
            {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no {}", path.display());
    }

    async fn assert_gone(pid: &str) {
        for _ in 0..200 {
            if !alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{pid} is still there");
    }

    #[tokio::test]
    async fn a_report_with_noise_around_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let invocation = fake(
            tmp.path(),
            r#"echo 'PHP Warning: hello'; echo '{"totals":{},"files":{},"errors":["x"]}'; echo 'bye'; echo 'on stderr' >&2; exit 1"#,
            LONG,
        );
        let report = run(&invocation, std::future::pending()).await.unwrap();
        assert_eq!(report.errors, ["x"]);
    }

    #[tokio::test]
    async fn no_report() {
        let tmp = tempfile::TempDir::new().unwrap();
        let invocation = fake(
            tmp.path(),
            "echo 'Note: Using x'; echo 'It is broken.' >&2; exit 3",
            LONG,
        );
        let failure = run(&invocation, std::future::pending()).await.unwrap_err();
        assert_eq!(failure.kind(), "report");
        let said = failure.describe(&invocation);
        assert!(
            said.contains("code 3") && said.contains("It is broken."),
            "{said}"
        );
        assert!(!said.contains("Note:"), "{said}");
    }

    #[tokio::test]
    async fn out_of_memory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let invocation = fake(
            tmp.path(),
            "echo 'PHP Fatal error:  Allowed memory size of 26214400 bytes exhausted' >&2; exit 255",
            LONG,
        );
        let failure = run(&invocation, std::future::pending()).await.unwrap_err();
        assert_eq!(failure.kind(), "memory");
        assert!(failure.describe(&invocation).contains("memoryLimit"));
    }

    #[tokio::test]
    async fn not_there() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut invocation = fake(tmp.path(), "", LONG);
        invocation.program = tmp.path().join("gone");
        let failure = run(&invocation, std::future::pending()).await.unwrap_err();
        assert_eq!(failure.kind(), "start");
    }

    #[tokio::test]
    async fn a_timeout_kills_it_and_what_it_started() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pids = tmp.path().join("pids");
        let invocation = fake(
            tmp.path(),
            &format!("sleep 1000 &\necho \"$! $$\" > {}\nwait", pids.display()),
            Duration::from_millis(300),
        );
        let result = run(&invocation, std::future::pending()).await;
        assert_eq!(result, Err(Failure::TimedOut(Duration::from_millis(300))));
        for pid in wait_for_file(&pids).await.split_whitespace() {
            assert_gone(pid).await;
        }
    }

    #[tokio::test]
    async fn cancelling_kills_it_and_what_it_started() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pids = tmp.path().join("pids");
        let invocation = fake(
            tmp.path(),
            &format!("sleep 1000 &\necho \"$! $$\" > {}\nwait", pids.display()),
            LONG,
        );
        let wrote = wait_for_file(&pids);
        let (result, written) = tokio::join!(
            run(&invocation, async {
                wait_for_file(&pids).await;
            }),
            wrote
        );
        assert_eq!(result, Err(Failure::Cancelled));
        for pid in written.split_whitespace() {
            assert_gone(pid).await;
        }
    }

    #[tokio::test]
    async fn dropping_a_run_kills_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pids = tmp.path().join("pids");
        let invocation = fake(
            tmp.path(),
            &format!("sleep 1000 &\necho \"$! $$\" > {}\nwait", pids.display()),
            LONG,
        );
        {
            let running = run(&invocation, std::future::pending());
            let _ = tokio::time::timeout(Duration::from_millis(300), running).await;
        }
        for pid in wait_for_file(&pids).await.split_whitespace() {
            assert_gone(pid).await;
        }
    }

    #[tokio::test]
    async fn what_it_leaves_behind_when_it_ends_goes_too() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pids = tmp.path().join("pids");
        let invocation = fake(
            tmp.path(),
            &format!(
                "sleep 1000 &\necho \"$!\" > {}\necho '{{\"totals\":{{}},\"files\":{{}},\"errors\":[]}}'",
                pids.display()
            ),
            LONG,
        );
        run(&invocation, std::future::pending()).await.unwrap();
        assert_gone(wait_for_file(&pids).await.trim()).await;
    }

    #[test]
    fn a_summary_is_of_what_matters() {
        let said = summary(
            "Note: Using configuration file /p/phpstan.neon.\n\nIn NeonCachedFileReader.php line 46:\n\n   Error while loading /p/bad.neon:  \n   Unexpected end.  \n\nanalyse [-c|--configuration CONFIGURATION]\n",
        );
        assert_eq!(said, "Error while loading /p/bad.neon: Unexpected end.");
    }
}
