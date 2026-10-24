// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit local hardware check; never used by normal CI tests.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = workstation_core::hardware::HardwareProfile::detect();
    println!("{}", serde_json::to_string_pretty(&snapshot)?);
    Ok(())
}
