//! Diagnostic: log in to the chat server (199.241.136.157:7005 as announced by system message 0x43) and dump events.
//!
//! ```text
//! cargo run --release -p ao-net --example chat_probe -- --char ID [--addr IP:PORT] [--wait SECS] [--out FILE]
//!                                                       [--say GROUP TEXT] [--tell NAME TEXT]
//! ```
//! Username and password are read from stdin (password without echo on a TTY); never from argv, never logged
//! (the login packet's key is redacted in every dump). One connection per run.

use ao_net::chat::{ChatCmd, ChatEvent, ChatSession};
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let (mut addr, mut char_id, mut wait, mut out) = ("199.241.136.157:7005".to_owned(), 0u32, 10u64, None::<String>);
    let (mut say, mut tell) = (None::<(String, String)>, None::<(String, String)>);
    while let Some(k) = a.next() {
        match k.as_str() {
            "--addr" => addr = a.next().unwrap_or_default(),
            "--char" => char_id = a.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--wait" => wait = a.next().and_then(|v| v.parse().ok()).unwrap_or(10),
            "--out" => out = a.next(),
            "--say" => say = Some((a.next().unwrap_or_default(), a.next().unwrap_or_default())),
            "--tell" => tell = Some((a.next().unwrap_or_default(), a.next().unwrap_or_default())),
            _ => anyhow::bail!("unknown argument {k} (credentials come from stdin)"),
        }
    }
    eprint!("username: ");
    let mut user = String::new();
    std::io::stdin().lock().read_line(&mut user)?;
    eprint!("password: ");
    let mut pw = String::new();
    std::io::stdin().lock().read_line(&mut pw)?;
    let (user, pw) = (user.trim(), pw.trim());
    let t0 = Instant::now();
    let tap: ao_net::conn::Tap = Box::new(move |sent, b| {
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        println!("{} {:>6}ms {hex}", if sent { ">" } else { "<" }, t0.elapsed().as_millis());
        if let Some(p) = &out {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{} {} {hex}", t0.elapsed().as_millis(), if sent { '>' } else { '<' });
            }
        }
    });
    let s = ChatSession::connect(addr.as_str(), Some(tap))?;
    s.login(user, pw, char_id);
    let mut groups: Vec<(ao_net::chat::GroupId, String)> = vec![];
    let mut sent = false;
    while t0.elapsed() < Duration::from_secs(wait) {
        while let Some(e) = s.poll() {
            println!("{:>6}ms {e:?}", t0.elapsed().as_millis());
            match e {
                ChatEvent::GroupJoin { group, name, .. } => groups.push((group, name)),
                ChatEvent::Disconnected(_) | ChatEvent::LoginFailed => return Ok(()),
                ChatEvent::Lookup { id, .. } if id != u32::MAX => {
                    if let Some((_, t)) = &tell {
                        s.send(ChatCmd::Tell { to: id, text: t.clone() });
                    }
                }
                _ => {}
            }
        }
        if !sent && t0.elapsed() > Duration::from_secs(4) {
            sent = true;
            if let Some((g, t)) = &say {
                match groups.iter().find(|(_, n)| n.eq_ignore_ascii_case(g)) {
                    Some((id, _)) => s.send(ChatCmd::Group { group: *id, text: t.clone() }),
                    None => println!("no group named {g}: {groups:?}"),
                }
            }
            if let Some((n, _)) = &tell {
                s.send(ChatCmd::Lookup(n.clone()));
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    s.send(ChatCmd::Quit);
    Ok(())
}
