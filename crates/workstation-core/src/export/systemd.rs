// SPDX-License-Identifier: GPL-3.0-or-later
//! A rootless `systemd --user` unit running `llama-server` with a profile's arguments.
//!
//! Pure text. The application never writes into the user's unit directory on its own, never
//! enables or starts the unit, and a unit the user starts is externally owned, not Managed.

use crate::runtime::profile::LaunchProfile;

/// Fixed launch alias: a unit has no supervisor to choose a random one.
const UNIT_ALIAS: &str = "00000000";

/// Quote one word for `ExecStart=`/`Environment=`: double quotes, with `\`, `"`, `%` and `$`
/// escaped so systemd specifier and variable expansion cannot alter it.
fn quote(word: &str) -> Result<String, String> {
    if word.chars().any(char::is_control) {
        return Err("Control characters cannot be written into a unit.".into());
    }
    let mut out = String::from("\"");
    for c in word.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

/// `program` is the absolute path of `llama-server`; its directory goes on the library path.
pub fn user_unit(profile: &LaunchProfile, program: &str) -> Result<String, String> {
    // systemd refuses an executable path with these characters even when quoted.
    if !program.starts_with('/')
        || program.split('/').any(|part| part == "..")
        || program
            .chars()
            .any(|c| c.is_control() || "\"'\\$%".contains(c))
    {
        return Err(
            "The llama-server path must be absolute, without '..', quotes, backslashes, '$' or '%'."
                .into(),
        );
    }
    let dir = program.rsplit_once('/').map_or("/", |(dir, _)| dir);
    let mut words = vec![quote(program)?];
    for arg in profile.arguments(UNIT_ALIAS)? {
        words.push(quote(&arg)?);
    }
    Ok(format!(
        "[Unit]\nDescription=llama.cpp server ({})\n\n[Service]\nType=simple\nEnvironment={}\nExecStart={}\nRestart=on-failure\nRestartSec=5\nNoNewPrivileges=yes\n\n[Install]\nWantedBy=default.target\n",
        profile.name,
        quote(&format!("LD_LIBRARY_PATH={dir}"))?,
        words.join(" "),
    ))
}
