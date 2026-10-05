//! Diagnostic: talk to a login server and hex-dump every frame.
//!
//! ```text
//! cargo run -p ao-net --example probe -- [--server IP:PORT] [--user NAME] [--version STR]
//!                                        [--wait SECS] [--bogus-credentials | --garbage-credentials] [--out FILE]
//! cargo run -p ao-net --example probe -- --login      # real login, prompts on the TTY
//! ```
//! Without `--server` the first server of the PRK status API is used. Default mode is
//! credential-free: UserLogin with a fake name, dump the reply, optionally (`--bogus-credentials`)
//! send UserCredentials built from a made-up password and dump the LoginError. `--login` prompts
//! for username and password on the terminal (password not echoed) and drives the real client.
//! The password is never accepted on the command line, never logged; UserCredentials frames are
//! redacted in every dump. One connection per run; no retries.

use ao_net::client::{fetch_servers, LoginEvent, LoginSession, ServerEntry};
use ao_net::conn::Conn;
use ao_net::crypto::make_challenge_response;
use ao_net::msg::Message;
use num_bigint::BigUint;
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

fn hexdump(sent: bool, b: &[u8]) -> String {
    let mut s = format!("{} {} bytes\n", if sent { ">>>" } else { "<<<" }, b.len());
    for (i, c) in b.chunks(16).enumerate() {
        let hex: Vec<String> = c.iter().map(|x| format!("{x:02x}")).collect();
        let asc: String = c.iter().map(|&x| if (0x20..0x7f).contains(&x) { x as char } else { '.' }).collect();
        s += &format!("{:04x}  {:<47}  {asc}\n", i * 16, hex.join(" "));
    }
    s
}

fn tap(out: Option<String>) -> ao_net::conn::Tap {
    Box::new(move |sent, b| {
        let d = hexdump(sent, b);
        print!("{d}");
        if let Some(p) = &out {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap();
            let _ = f.write_all(d.as_bytes());
        }
    })
}

fn prompt_password() -> String {
    // termios: disable ECHO while reading one line from the TTY.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        let have = libc::tcgetattr(0, &mut t) == 0;
        let saved = t;
        if have {
            t.c_lflag &= !libc::ECHO;
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
        eprint!("password: ");
        let mut l = String::new();
        let _ = std::io::stdin().lock().read_line(&mut l);
        if have {
            libc::tcsetattr(0, libc::TCSANOW, &saved);
        }
        eprintln!();
        l.trim_end_matches(['\r', '\n']).to_owned()
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let (mut server, mut user, mut version) = (None, "aomac-probe".to_owned(), "00.7.2_EP1".to_owned());
    let (mut wait, mut bogus, mut garbage, mut login, mut out) = (5u64, false, false, false, None);
    while let Some(k) = a.next() {
        match k.as_str() {
            "--server" => server = a.next(),
            "--user" => user = a.next().unwrap_or_default(),
            "--version" => version = a.next().unwrap_or_default(),
            "--wait" => wait = a.next().and_then(|v| v.parse().ok()).unwrap_or(5),
            "--bogus-credentials" => bogus = true,
            "--garbage-credentials" => garbage = true,
            "--login" => login = true,
            "--out" => out = a.next(),
            _ => anyhow::bail!("unknown argument {k} (passwords are never accepted as arguments)"),
        }
    }
    let entry = match server {
        Some(s) => {
            let sa: SocketAddr = s.parse()?;
            let SocketAddr::V4(v4) = sa else { anyhow::bail!("IPv4 only") };
            ServerEntry { name: s, ip: *v4.ip(), port: v4.port(), players: 0 }
        }
        None => {
            let l = fetch_servers()?;
            println!("servers: {l:?}");
            l.into_iter().next().ok_or_else(|| anyhow::anyhow!("empty server list"))?
        }
    };
    let addr = SocketAddr::from((entry.ip, entry.port));

    if login {
        eprint!("username: ");
        let mut u = String::new();
        std::io::stdin().lock().read_line(&mut u)?;
        let pw = prompt_password();
        let s = LoginSession::connect_traced(&entry, tap(out))?;
        s.login(u.trim(), &pw);
        let end = Instant::now() + Duration::from_secs(wait.max(60));
        while Instant::now() < end {
            while let Some(ev) = s.poll() {
                println!("{ev:?}");
                if matches!(ev, LoginEvent::Disconnected(_) | LoginEvent::LoginError { .. } | LoginEvent::Rejected { .. }) {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        return Ok(());
    }

    let mut c = Conn::connect(addr)?;
    c.tap = Some(tap(out));
    println!("connected {addr}");
    c.send_message(&Message::UserLogin { protocol: 2, name: user.clone(), client_version: version })?;
    let end = Instant::now() + Duration::from_secs(wait);
    let mut salt = None;
    while Instant::now() < end {
        match c.recv(end - Instant::now()) {
            Ok(Some(f)) => {
                println!("frame ptype={:#x} seq={} sender={:#x} receiver={:#x} payload={}B", f.ptype, f.seq, f.sender, f.receiver, f.payload.len());
                match Message::from_frame(&f) {
                    Ok(m) => {
                        println!("  {m:?}");
                        if let Message::ServerSalt(s) = m {
                            salt = Some(s);
                            if garbage {
                                // not a decryptable response: tells how the server treats undecryptable input
                                let response = "abcd-0011223344556677".to_owned();
                                c.send_message(&Message::UserCredentials { name: user.clone(), response })?;
                            } else if bogus {
                                let mut e = [0u8; 16];
                                getrandom::getrandom(&mut e)?;
                                let mut p = [0u8; 8];
                                getrandom::getrandom(&mut p)?;
                                let r = make_challenge_response(&user, &s, "not-a-real-password", &BigUint::from_bytes_be(&e), p)?;
                                c.send_message(&Message::UserCredentials { name: user.clone(), response: r })?;
                            }
                        }
                    }
                    Err(e) => println!("  undecoded: {e}"),
                }
            }
            Ok(None) => break,
            Err(e) => {
                println!("ended: {e}");
                break;
            }
        }
    }
    println!("salt: {:02x?}", salt);
    Ok(())
}
