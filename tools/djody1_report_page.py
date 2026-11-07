"""DESKTOP-JODY1 item 1: Report a problem is a GitHub-issue how-to (Jody, 11/03).

Removes the feedback-address setting and its placeholder, adds the host's version/OS to the app
session, and exposes one native setter the Electron main process calls at start-up.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace(path, old, new, count=1):
    p = ROOT / path
    t = p.read_text()
    assert t.count(old) == count, (path, old, t.count(old))
    p.write_text(t.replace(old, new))


# The shell no longer carries a report address: no setting, no placeholder, no email path.
replace('crates/fb-templates/src/vm.rs', '''    pub fb_banner: String,
    pub fb_feedback_url: String,
}''', '''    pub fb_banner: String,
}''')
replace('crates/fb-app/src/screens.rs', '''        fb_banner: ALPHA_BANNER.into(),
        fb_feedback_url: String::new(),
''', '''        fb_banner: ALPHA_BANNER.into(),
''')
replace('crates/fb-templates/templates/ledger/base.html',
        '''<a class="report-problem" href="{% if fb_feedback_url %}{{ fb_feedback_url }}{% else %}{{ url('report_problem') }}{% endif %}"{% if fb_feedback_url %} rel="noopener noreferrer" target="_blank"{% endif %}>Report a problem{% if fb_feedback_url %}<span class="sr-only"> (opens in a new tab)</span>{% endif %}</a>''',
        '''<a class="report-problem" href="{{ url('report_problem') }}">Report a problem</a>''')

(ROOT / 'crates/fb-templates/templates/ledger/report_problem.html').write_text('''{% extends "ledger/base.html" %}
{% block content %}
<h1>Report a problem</h1>
<p>FreedomBooks is an alpha. Keep your real books in your current software.</p>
<p>Problems are reported as issues on GitHub, the website where the FreedomBooks project lives. You need a free GitHub account.</p>
<p class="notice" role="note"><strong>GitHub issues are public.</strong> Anyone can read them. Never paste real financial data, customer names, passwords, keys or bank details.</p>
<ol class="report-steps">
<li><a class="report-issue-link" href="{{ issue_url }}">Open the GitHub issue page</a> (it opens in your web browser). You can also copy the address: <code>{{ issue_url }}</code></li>
<li>Write the steps you took, one at a time, starting from Home.</li>
<li>Say what you expected to happen.</li>
<li>Say what actually happened. Copy any error message word for word.</li>
<li>Copy your FreedomBooks version and operating system into the issue:
<pre class="report-details">FreedomBooks version: {{ about.version }}
Operating system: {{ about.os }}</pre></li>
<li>Attach a screenshot only after covering names, amounts and account numbers.</li>
</ol>
{% endblock %}
''')

replace('crates/fb-app/src/screens/d2.rs', '''        "report_problem" => {
            let vm = ctx(app, route)?;
            Ok((draw(app, "ledger/report_problem.html", &vm)?, 200))''', '''        "report_problem" => {
            let mut vm = ctx(app, route)?;
            vm["issue_url"] = json!(crate::session::ISSUE_URL);
            vm["about"] = json!({"version": app.host.version, "os": app.host.os});
            Ok((draw(app, "ledger/report_problem.html", &vm)?, 200))''')

replace('crates/fb-app/src/session.rs', '''pub struct App {
    pub(crate) d7a_reviews''', '''/// The one outside address the app links to: the project's new-issue chooser. The Electron main process
/// opens exactly this URL in the system browser (`electron/main/guards.cjs` `EXTERNAL_URLS`).
pub const ISSUE_URL: &str = "https://github.com/CyberJody/FreedomBooks/issues/new/choose";

/// What "Report a problem" shows for copying into an issue: the app's own version and the host's
/// operating-system name/version, nothing else about the machine. The desktop shell sets both at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub version: String,
    pub os: String,
}

impl Host {
    /// Plain one-line text: control/format characters dropped, whitespace collapsed, at most 100
    /// characters, and "Unknown" when nothing is left.
    pub fn new(version: &str, os: &str) -> Host {
        fn clean(s: &str) -> String {
            let kept: String = s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !fb_core::py::is_control_or_format(*c)).collect();
            let text: String = kept.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(100).collect();
            if text.trim().is_empty() { "Unknown".into() } else { text.trim_end().into() }
        }
        Host { version: clean(version), os: clean(os) }
    }
}

impl Default for Host {
    /// Before the shell reports in (tests, tools): the core's own version and the OS family.
    fn default() -> Host {
        let os = match std::env::consts::OS {
            "linux" => "Linux",
            "macos" => "macOS",
            "windows" => "Windows",
            other => other,
        };
        Host::new(env!("CARGO_PKG_VERSION"), os)
    }
}

pub struct App {
    pub host: Host,
    pub(crate) d7a_reviews''')
replace('crates/fb-app/src/session.rs', '''        App {
            d7a_reviews: Default::default(),''', '''        App {
            host: Host::default(),
            d7a_reviews: Default::default(),''')
replace('crates/fb-app/src/lib.rs', 'pub use session::{App, AppError, AppResult};',
        'pub use session::{App, AppError, AppResult, Host, ISSUE_URL};')

replace('crates/fb-node/src/lib.rs', '''/// `{"open": bool}`.
#[napi]''', '''/// The app version (from the shell's build metadata) and the host OS name/version, shown on
/// "Report a problem" for copying into an issue. Plain text only; fb-app cleans and caps both.
#[napi]
pub fn set_host(version: String, os: String) -> Result<()> {
    app()?.host = fb_app::Host::new(&version, &os);
    Ok(())
}

/// `{"open": bool}`.
#[napi]''')
print('ok')
