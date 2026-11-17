// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use std::io::{self, Read};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 || !["--dump", "--dump-utf8", "--dump-meta"].contains(&args[1].as_str()) {
        return Err("Usage: utty-capture --dump-utf8|--dump-meta COLS ROWS".into());
    }
    let mut bytes = Vec::new();
    io::stdin()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    print!(
        "{}",
        utty::core::dump(
            &bytes,
            args[2].parse()?,
            args[3].parse()?,
            args[1] == "--dump-meta"
        )?
    );
    Ok(())
}
