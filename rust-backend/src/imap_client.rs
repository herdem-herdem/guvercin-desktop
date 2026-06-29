use imap;
use native_tls::TlsConnector;
use std::io::{Read, Write};
use std::net::TcpStream;

use crate::i18n::tr;
use tracing::warn;

pub async fn authorize(
    server: &str,
    email: &str,
    password: &str,
    port: u16,
    verify_ssl: bool,
    ssl_mode: &str,
) -> (bool, String) {
    let server = server.to_string();
    let email = email.to_string();
    let password = password.to_string();
    let ssl_mode = ssl_mode.to_string();

    tokio::task::spawn_blocking(move || {
        inner_authorize(&server, &email, &password, port, verify_ssl, &ssl_mode)
    })
    .await
    .unwrap_or_else(|e| (false, tr(&format!("Unexpected error: {e}"))))
}

pub async fn preview_mailboxes(
    server: &str,
    email: &str,
    password: &str,
    port: u16,
    verify_ssl: bool,
    ssl_mode: &str,
) -> Result<Vec<String>, String> {
    let server = server.to_string();
    let email = email.to_string();
    let password = password.to_string();
    let ssl_mode = ssl_mode.to_string();

    tokio::task::spawn_blocking(move || {
        inner_preview_mailboxes(&server, &email, &password, port, verify_ssl, &ssl_mode)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Unexpected error: {e}")))
}

pub async fn probe_server(
    imap_server: &str,
    imap_port: u16,
    smtp_server: Option<&str>,
    smtp_port: Option<u16>,
    ssl_mode: &str,
) -> Result<(), String> {
    let imap_server = imap_server.to_string();
    let smtp_server = smtp_server.map(|s| s.to_string());
    let ssl_mode = ssl_mode.to_string();

    tokio::task::spawn_blocking(move || {
        probe_single_server(&imap_server, imap_port, &ssl_mode)?;
        if let Some(smtp_server) = smtp_server.as_deref() {
            if let Some(smtp_port) = smtp_port {
                probe_tcp_port(smtp_server, smtp_port)?;
            }
        }
        Ok(())
    })
    .await
    .unwrap_or_else(|e| Err(format!("Unexpected error: {e}")))
}

fn probe_single_server(server: &str, port: u16, ssl_mode: &str) -> Result<(), String> {
    let mut builder = TlsConnector::builder();
    builder.danger_accept_invalid_certs(true);
    builder.danger_accept_invalid_hostnames(true);

    let tls = builder.build().map_err(|e| format!("{e}"))?;

    match ssl_mode.to_uppercase().as_str() {
        "SSL" => {
            let _ = imap::connect((server, port), server, &tls).map_err(|e| format!("{e}"))?;
            Ok(())
        }
        "STARTTLS" => {
            let _ =
                imap::connect_starttls((server, port), server, &tls).map_err(|e| format!("{e}"))?;
            Ok(())
        }
        _ => {
            let stream = TcpStream::connect((server, port)).map_err(|e| format!("{e}"))?;
            let mut client = imap::Client::new(stream);
            client.read_greeting().map_err(|e| format!("{e}"))?;
            Ok(())
        }
    }
}

fn probe_tcp_port(server: &str, port: u16) -> Result<(), String> {
    let stream = TcpStream::connect((server, port)).map_err(|e| format!("{e}"))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(10)));
    Ok(())
}

fn inner_authorize(
    server: &str,
    email: &str,
    password: &str,
    port: u16,
    verify_ssl: bool,
    ssl_mode: &str,
) -> (bool, String) {
    let mut builder = TlsConnector::builder();
    if !verify_ssl {
        #[allow(deprecated)]
        {
            builder.danger_accept_invalid_certs(true);
            builder.danger_accept_invalid_hostnames(true);
        }
    }

    let tls = match builder.build() {
        Ok(t) => t,
        Err(e) => {
            return (false, tr(&format!("Unexpected error: {e}")));
        }
    };

    match ssl_mode.to_uppercase().as_str() {
        "SSL" => match imap::connect((server, port), server, &tls) {
            Ok(client) => perform_auth(client, email, password),
            Err(e) => {
                warn!("IMAP SSL connect failed for {}:{} - {}", server, port, e);
                (false, tr(&format!("Unexpected error: {e}")))
            }
        },
        "STARTTLS" => match imap::connect_starttls((server, port), server, &tls) {
            Ok(client) => perform_auth(client, email, password),
            Err(e) => {
                warn!(
                    "IMAP STARTTLS connect failed for {}:{} - {}",
                    server, port, e
                );
                (false, tr(&format!("Unexpected error: {e}")))
            }
        },
        _ => match std::net::TcpStream::connect((server, port)) {
            Ok(stream) => {
                let mut client = imap::Client::new(stream);
                if let Err(e) = client.read_greeting() {
                    warn!("IMAP greeting failed for {}:{} - {}", server, port, e);
                    return (false, tr(&format!("Unexpected error: {e}")));
                }
                perform_auth(client, email, password)
            }
            Err(e) => {
                warn!("TCP connect to IMAP failed for {}:{} - {}", server, port, e);
                (false, tr(&format!("Unexpected error: {e}")))
            }
        },
    }
}

fn perform_auth<T: Read + Write>(
    client: imap::Client<T>,
    email: &str,
    password: &str,
) -> (bool, String) {
    let mut session = match client.login(email, password) {
        Ok(s) => s,
        Err((imap::error::Error::No(resp), _)) => {
            return (false, tr(&format!("Login failed: {}", resp)));
        }
        Err((imap::error::Error::Bad(resp), _)) => {
            return (
                false,
                tr(&format!("Invalid command or protocol error: {}", resp)),
            );
        }
        Err((e, _)) => {
            return (false, tr(&format!("Unexpected error: {e}")));
        }
    };

    let _ = session.logout();

    (true, tr("Authorization successful."))
}

fn inner_preview_mailboxes(
    server: &str,
    email: &str,
    password: &str,
    port: u16,
    verify_ssl: bool,
    ssl_mode: &str,
) -> Result<Vec<String>, String> {
    let mut builder = TlsConnector::builder();
    if !verify_ssl {
        #[allow(deprecated)]
        {
            builder.danger_accept_invalid_certs(true);
            builder.danger_accept_invalid_hostnames(true);
        }
    }

    let tls = builder
        .build()
        .map_err(|e| format!("Unexpected error: {e}"))?;

    match ssl_mode.to_uppercase().as_str() {
        "SSL" => {
            let client = imap::connect((server, port), server, &tls).map_err(|e| {
                warn!("IMAP SSL probe failed for {}:{} - {}", server, port, e);
                format!("Unexpected error: {e}")
            })?;
            perform_preview(client, email, password)
        }
        "STARTTLS" => {
            let client = imap::connect_starttls((server, port), server, &tls).map_err(|e| {
                warn!("IMAP STARTTLS probe failed for {}:{} - {}", server, port, e);
                format!("Unexpected error: {e}")
            })?;
            perform_preview(client, email, password)
        }
        _ => {
            let stream = std::net::TcpStream::connect((server, port)).map_err(|e| {
                warn!("TCP probe to IMAP failed for {}:{} - {}", server, port, e);
                format!("Unexpected error: {e}")
            })?;
            let mut client = imap::Client::new(stream);
            client.read_greeting().map_err(|e| {
                warn!("IMAP greeting probe failed for {}:{} - {}", server, port, e);
                format!("Unexpected error: {e}")
            })?;
            perform_preview(client, email, password)
        }
    }
}

fn perform_preview<T: Read + Write>(
    client: imap::Client<T>,
    email: &str,
    password: &str,
) -> Result<Vec<String>, String> {
    let mut session = client
        .login(email, password)
        .map_err(|(e, _)| format!("{e}"))?;
    let list = session
        .list(Some(""), Some("*"))
        .map_err(|e| format!("list mailbox failed: {e}"))?;
    let mailboxes = list.iter().map(|n| n.name().to_string()).collect();
    let _ = session.logout();
    Ok(mailboxes)
}
