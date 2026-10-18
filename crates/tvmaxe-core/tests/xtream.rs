//! Updating an Xtream Codes subscription against a local fake panel.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use tvmaxe_core::formats::xtream::Account;
use tvmaxe_core::{Database, Kind, net};

/// A panel that answers like Xtream Codes; it records the requests it gets.
struct Panel {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

fn panel(handler: impl Fn(&str) -> (u16, String) + Send + 'static) -> Panel {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let target = request
                .lines()
                .next()
                .and_then(|l| l.split(' ').nth(1))
                .unwrap_or("")
                .to_string();
            log.lock().unwrap().push(target.clone());
            let (status, body) = handler(&target);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    Panel { base, requests }
}

fn good_panel(user: &'static str, password: &'static str) -> Panel {
    panel(move |target| {
        let creds = format!("username={user}&password={password}");
        if !target.contains(&creds) {
            return (200, r#"{"user_info":{"auth":0}}"#.into());
        }
        let body = if target.contains("action=get_live_categories") {
            r#"[{"category_id":"1","category_name":"Romania"},
                {"category_id":"2","category_name":"Radio"}]"#
        } else if target.contains("action=get_live_streams") {
            r#"[{"name":"Pro TV","stream_type":"live","stream_id":1,"category_id":"1",
                 "epg_channel_id":"protv.ro","stream_icon":"http://l.example/p.png"},
                {"name":"Antena 1","stream_type":"live","stream_id":2,"category_id":"1"},
                {"name":"Radio Uno","stream_type":"live","stream_id":3,"category_id":"2"}]"#
        } else {
            r#"{"user_info":{"auth":1,"status":"Active","allowed_output_formats":["ts","m3u8"]},
                "server_info":{}}"#
        };
        (200, body.to_string())
    })
}

struct TempDb(std::path::PathBuf);

impl TempDb {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tvmaxe-xtream-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.join("tvmaxe.db"))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn subscribe(db: &TempDb, account: &Account) -> i64 {
    Database::open(&db.0)
        .unwrap()
        .add_subscription(&account.host(), &account.source_url())
        .unwrap()
}

#[test]
fn channels_groups_and_sources_are_stored() {
    let p = good_panel("ana", "s3cret");
    let account = Account::new(&p.base, "ana", "s3cret").unwrap();
    let db = TempDb::new("ok");
    let id = subscribe(&db, &account);

    assert_eq!(net::update_subscription(&db.0, id).unwrap(), 3);

    let db = Database::open(&db.0).unwrap();
    let tv = db.channels(Kind::Tv).unwrap();
    let radio = db.channels(Kind::Radio).unwrap();
    assert_eq!(
        tv.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Pro TV", "Antena 1"]
    );
    assert_eq!(radio.len(), 1);
    assert_eq!(tv[0].group.as_deref(), Some("Romania"));
    assert_eq!(
        tv[0].urls,
        [
            format!("{}/live/ana/s3cret/1.ts", p.base),
            format!("{}/live/ana/s3cret/1.m3u8", p.base)
        ]
    );
    let sub = &db.subscriptions().unwrap()[0];
    assert_eq!(sub.last_error, None);
    assert_eq!((sub.tv_count, sub.radio_count), (2, 1));
    assert!(
        !sub.display_url().contains("s3cret"),
        "{}",
        sub.display_url()
    );
}

#[test]
fn the_account_is_checked_before_the_channels_are_asked_for() {
    let p = good_panel("ana", "s3cret");
    let account = Account::new(&p.base, "ana", "s3cret").unwrap();
    let db = TempDb::new("order");
    let id = subscribe(&db, &account);
    net::update_subscription(&db.0, id).unwrap();
    let requests = p.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(!requests[0].contains("action="), "{requests:?}");
    assert!(requests[1].contains("get_live_categories"), "{requests:?}");
    assert!(requests[2].contains("get_live_streams"), "{requests:?}");
}

#[test]
fn a_wrong_password_is_reported_and_kept_in_the_database() {
    let p = good_panel("ana", "s3cret");
    let account = Account::new(&p.base, "ana", "hunter2x").unwrap();
    let db = TempDb::new("wrong");
    let id = subscribe(&db, &account);

    let err = net::update_subscription(&db.0, id).unwrap_err().to_string();
    assert_eq!(err, tvmaxe_core::i18n::tr("wrong user name or password"));
    let stored = Database::open(&db.0).unwrap().subscriptions().unwrap();
    assert_eq!(stored[0].last_error.as_deref(), Some(err.as_str()));
    // Nothing else is asked for after the refusal.
    assert_eq!(p.requests.lock().unwrap().len(), 1);
    // The error never shows the password.
    assert!(!err.contains("hunter2x"), "{err}");
}

#[test]
fn something_that_is_not_a_panel_is_reported() {
    let p = panel(|_| (200, "<html>Welcome</html>".into()));
    let account = Account::new(&p.base, "u", "p").unwrap();
    let db = TempDb::new("html");
    let id = subscribe(&db, &account);
    let err = net::update_subscription(&db.0, id).unwrap_err().to_string();
    assert_eq!(
        err,
        tvmaxe_core::i18n::tr("the server did not answer like an Xtream Codes panel")
    );
}

#[test]
fn an_account_without_channels_is_an_empty_list() {
    let p = panel(|target| {
        let body = if target.contains("action=") {
            "[]"
        } else {
            r#"{"user_info":{"auth":1,"status":"Active"}}"#
        };
        (200, body.into())
    });
    let account = Account::new(&p.base, "u", "p").unwrap();
    let db = TempDb::new("empty");
    let id = subscribe(&db, &account);
    assert!(matches!(
        net::update_subscription(&db.0, id),
        Err(tvmaxe_core::Error::Empty)
    ));
}

#[test]
fn a_panel_that_is_down_is_a_download_error() {
    // Nothing listens on this port.
    let account = Account::new("http://127.0.0.1:9", "u", "p").unwrap();
    let db = TempDb::new("down");
    let id = subscribe(&db, &account);
    let err = net::update_subscription(&db.0, id).unwrap_err();
    assert!(matches!(err, tvmaxe_core::Error::Http(_)), "{err}");
    assert!(!err.to_string().contains("password"), "{err}");
}
