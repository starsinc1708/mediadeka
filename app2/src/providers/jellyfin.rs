pub(super) mod catalog;
mod collection;
mod download;
pub(super) mod likes;
mod playlist;
#[cfg(all(test, windows))]
mod recovery_tests;

use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use url::Url;

use super::{
    Credentials, ErrorKind, Home, HomeItem, HomeSection, HomeSectionStyle, LoginPrompt, Profile,
    ProviderError, ProviderId, Session, transport::*,
};

pub(super) use collection::read_collection;
pub(super) use download::original;
pub(super) use playlist::{playlist, track_stream, valid_item_id};

const PROVIDER: ProviderId = ProviderId::Jellyfin;
const QUICK_INTERVAL: Duration = Duration::from_secs(3);

fn server(input: &str) -> Result<Url, ProviderError> {
    let mut url = Url::parse(input.trim())
        .ok()
        .filter(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
        .ok_or_else(|| {
            ProviderError::new(
                ErrorKind::InvalidInput,
                "Укажите адрес Jellyfin с http:// или https://.",
            )
        })?;
    if !url.path().ends_with('/') {
        url.path_segments_mut()
            .expect("hierarchical HTTP address")
            .push("");
    }
    Ok(url)
}

fn at(server: &Url, endpoint: &str) -> Url {
    server.join(endpoint).expect("constant Jellyfin endpoint")
}

fn auth(device: &str, token: Option<&str>) -> Result<String, ProviderError> {
    if !valid_token(device, 128) || token.is_some_and(|value| !valid_token(value, 2048)) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Сохранённое подключение Jellyfin недействительно.",
        ));
    }
    let mut value = format!(
        "MediaBrowser Client=\"Mediadeka\", Device=\"Windows\", DeviceId=\"{device}\", Version=\"{}\"",
        env!("CARGO_PKG_VERSION")
    );
    if let Some(token) = token {
        value.push_str(&format!(", Token=\"{token}\""));
    }
    Ok(value)
}

pub fn sign_in(address: &str, username: &str, password: &str) -> Result<Session, ProviderError> {
    let server = server(address)?;
    let username = username.trim();
    if username.is_empty() || username.len() > 256 || password.len() > 1024 {
        return Err(ProviderError::new(
            ErrorKind::InvalidInput,
            "Укажите имя пользователя Jellyfin.",
        ));
    }
    let device = random()?;
    let response = agent()
        .post(at(&server, "Users/AuthenticateByName").as_str())
        .header("Authorization", auth(&device, None)?)
        .send_json(json!({"Username": username, "Pw": password}));
    let result: Value = decode(PROVIDER, response).map_err(|error| {
        if error.kind == ErrorKind::Expired {
            ProviderError::new(
                ErrorKind::Rejected,
                "Jellyfin отклонил имя пользователя или пароль.",
            )
        } else {
            error
        }
    })?;
    confirmed(server, device, &result)
}

pub fn quick_connect(
    address: &str,
    cancelled: &AtomicBool,
    prompt: impl Fn(LoginPrompt),
) -> Result<Session, ProviderError> {
    let server = server(address)?;
    let agent = agent();
    let device = random()?;
    let header = auth(&device, None)?;
    let started: Value = decode(
        PROVIDER,
        agent
            .post(at(&server, "QuickConnect/Initiate").as_str())
            .header("Authorization", &header)
            .send_empty(),
    )
    .map_err(|error| {
        if matches!(
            error.kind,
            ErrorKind::Rejected | ErrorKind::Unavailable | ErrorKind::Expired
        ) {
            ProviderError::new(
                ErrorKind::Unavailable,
                format!(
                    "Quick Connect недоступен. {} Можно войти с паролем.",
                    error.message
                ),
            )
        } else {
            error
        }
    })?;
    let code = field(&started, "Code").ok_or_else(|| invalid_response(PROVIDER))?;
    let secret = field(&started, "Secret").ok_or_else(|| invalid_response(PROVIDER))?;
    check_cancelled(cancelled)?;
    prompt(LoginPrompt::Code {
        code,
        url: at(&server, "web/index.html#!/quickconnect").into(),
    });
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    loop {
        wait(cancelled, QUICK_INTERVAL, deadline)?;
        let status: Value = decode(
            PROVIDER,
            agent
                .get(at(&server, "QuickConnect/Connect").as_str())
                .header("Authorization", &header)
                .query("secret", &secret)
                .call(),
        )?;
        if status["Authenticated"] == true {
            break;
        }
    }
    check_cancelled(cancelled)?;
    let result = decode(
        PROVIDER,
        agent
            .post(at(&server, "Users/AuthenticateWithQuickConnect").as_str())
            .header("Authorization", &header)
            .send_json(json!({"Secret": secret})),
    )?;
    confirmed(server, device, &result)
}

fn confirmed(server: Url, device: String, result: &Value) -> Result<Session, ProviderError> {
    let access_token = field(result, "AccessToken").ok_or_else(|| invalid_response(PROVIDER))?;
    restore(Credentials::Jellyfin {
        server: server.into(),
        device,
        access_token,
    })
}

fn me(server: &Url, device: &str, token: &str) -> Result<(String, String), ProviderError> {
    let response = agent()
        .get(at(server, "Users/Me").as_str())
        .header("Authorization", auth(device, Some(token))?)
        .call()
        .map_err(|error| network(PROVIDER, error))?;
    match response.status().as_u16() {
        401 => {
            // Jellyfin uses the same empty response after logout and for an unknown token.
            return Err(ProviderError::new(
                ErrorKind::Expired,
                "Jellyfin не подтвердил сохранённый вход. Войдите снова.",
            ));
        }
        403 => {
            // A reverse proxy can forbid the request without validating the Jellyfin token.
            return Err(ProviderError::new(
                ErrorKind::Unavailable,
                "Не удалось проверить профиль Jellyfin (HTTP 403). Проверьте сервер и повторите.",
            ));
        }
        _ => {}
    }
    let result: Value = decode(PROVIDER, Ok(response)).map_err(|error| {
        if error.kind == ErrorKind::InvalidResponse {
            profile_unavailable()
        } else {
            error
        }
    })?;
    let id = field(&result, "Id")
        .filter(|id| valid_item_id(id))
        .ok_or_else(profile_unavailable)?;
    let name = field(&result, "Name").ok_or_else(profile_unavailable)?;
    Ok((id, name))
}

fn profile_unavailable() -> ProviderError {
    ProviderError::new(
        ErrorKind::Unavailable,
        "Jellyfin временно не вернул данные профиля. Повторите проверку.",
    )
}

pub fn restore(credentials: Credentials) -> Result<Session, ProviderError> {
    let Credentials::Jellyfin {
        server: address,
        device,
        access_token,
    } = &credentials
    else {
        unreachable!()
    };
    let server = server(address)?;
    let (id, name) = me(&server, device, access_token)?;
    let id = format!("{server}#{id}");
    Ok(Session::new(Profile { id, name }, credentials))
}

pub(super) fn home(
    address: &str,
    device: &str,
    token: &str,
    profile_id: &str,
) -> Result<Home, ProviderError> {
    let server = server(address)?;
    let user = profile_user(&server, profile_id)?;
    let header = auth(device, Some(token))?;
    let agent = agent();
    let (audio, playlists) = std::thread::scope(|scope| {
        let audio = scope.spawn(|| home_items(&agent, &server, &header, user, "Audio"));
        let playlists = home_items(&agent, &server, &header, user, "Playlist");
        (audio.join().expect("Jellyfin audio shelf"), playlists)
    });
    let title = "Недавно добавленная музыка";
    let mut items = audio?;
    let mut sections = Vec::new();
    let playlists = playlists.unwrap_or_default();
    if !playlists.is_empty() {
        if !items.is_empty() {
            sections.push(HomeSection {
                title: title.into(),
                len: items.len(),
                ..Default::default()
            });
        }
        sections.push(HomeSection {
            title: "Плейлисты Jellyfin".into(),
            start: items.len(),
            len: playlists.len(),
            style: HomeSectionStyle::Collections,
            ..Default::default()
        });
        items.extend(playlists);
    }
    Ok(Home {
        title: title.into(),
        items,
        sections,
    })
}

fn profile_user<'a>(server: &Url, profile_id: &'a str) -> Result<&'a str, ProviderError> {
    profile_id
        .split_once('#')
        .filter(|(address, id)| *address == server.as_str() && valid_item_id(id))
        .map(|(_, id)| id)
        .ok_or_else(|| invalid_response(PROVIDER))
}

fn home_items(
    agent: &ureq::Agent,
    server: &Url,
    header: &str,
    user: &str,
    kind: &str,
) -> Result<Vec<HomeItem>, ProviderError> {
    let value: Value = decode(
        PROVIDER,
        agent
            .get(at(server, "Items").as_str())
            .header("Authorization", header)
            .query("UserId", user)
            .query("Recursive", "true")
            .query("IncludeItemTypes", kind)
            .query("SortBy", "DateCreated")
            .query("SortOrder", "Descending")
            .query("Limit", HOME_LIMIT.to_string())
            .call(),
    )?;
    Ok(value["Items"]
        .as_array()
        .ok_or_else(|| invalid_response(PROVIDER))?
        .iter()
        .filter_map(|row| {
            let id = field(row, "Id").filter(|id| valid_item_id(id))?;
            let is_playlist = kind == "Playlist";
            Some(HomeItem {
                id: if is_playlist {
                    format!("playlist:{id}")
                } else {
                    id.clone()
                },
                title: field(row, "Name")?,
                subtitle: if is_playlist {
                    "Плейлист".into()
                } else {
                    row["Artists"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                url: is_playlist.then(|| playlist::link(server, &id)),
                artwork: None,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, ErrorKind as IoErrorKind, Write},
        net::{TcpListener, TcpStream},
        thread,
    };

    use super::*;

    #[test]
    fn preserves_server_subpath_and_rejects_embedded_credentials() {
        assert_eq!(
            at(&server("https://example.com/media").unwrap(), "Users/Me").as_str(),
            "https://example.com/media/Users/Me"
        );
        assert!(server("https://user:pass@example.com/media").is_err());
        assert!(server("https://example.com/media?other=1").is_err());
    }

    #[test]
    fn jellyfin_home_reuses_confirmed_user_for_both_catalog_requests() {
        const USER: &str = "9a94ac325cdf47c08cb748b21ab8817c";
        const PROFILE_PATH: &str = "/jellyfin/Users/Me";
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}/jellyfin/", listener.local_addr().unwrap());
        let requests = thread::scope(|scope| {
            let server = scope.spawn(move || {
                (0..3)
                    .map(|_| {
                        let mut stream = accept(&listener);
                        let target = request_target(&mut stream);
                        let body = if target == PROFILE_PATH {
                            json!({"Id": USER, "Name": "Listener"})
                        } else {
                            json!({"Items": []})
                        }
                        .to_string();
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .unwrap();
                        target
                    })
                    .collect::<Vec<_>>()
            });
            let restored = restore(Credentials::Jellyfin {
                server: address.clone(),
                device: "fixture-device".into(),
                access_token: "fixture-token".into(),
            })
            .unwrap();
            let home = super::super::home(&restored, &AtomicBool::new(false)).unwrap();
            assert!(home.items.is_empty());
            server.join().unwrap()
        });
        assert_eq!(requests[0], PROFILE_PATH);
        let mut kinds = Vec::new();
        for target in &requests[1..] {
            let url = Url::parse(&address).unwrap().join(target).unwrap();
            assert_eq!(url.path(), "/jellyfin/Items");
            let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
            assert_eq!(query["UserId"], USER);
            kinds.push(query["IncludeItemTypes"].to_string());
        }
        kinds.sort_unstable();
        assert_eq!(kinds, ["Audio", "Playlist"]);
    }

    #[test]
    fn jellyfin_profile_denial_does_not_confuse_an_invalid_token_with_a_server_failure() {
        for (status, expected) in [
            ("401 Unauthorized", ErrorKind::Expired),
            ("403 Forbidden", ErrorKind::Unavailable),
            ("503 Service Unavailable", ErrorKind::Network),
        ] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = format!("http://{}/", listener.local_addr().unwrap());
            thread::scope(|scope| {
                let probe = scope.spawn(move || {
                    let mut stream = accept(&listener);
                    assert_eq!(request_target(&mut stream), "/Users/Me");
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                });
                let restored = restore(Credentials::Jellyfin {
                    server: address,
                    device: "fixture-device".into(),
                    access_token: "fixture-token".into(),
                });
                assert!(matches!(restored, Err(error) if error.kind == expected));
                probe.join().unwrap();
            });
        }
    }

    fn accept(listener: &TcpListener) -> TcpStream {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == IoErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "Jellyfin request did not arrive");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("Jellyfin listener: {error}"),
            }
        }
    }

    fn request_target(stream: &mut TcpStream) -> String {
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let target = line.split_whitespace().nth(1).unwrap().to_owned();
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                return target;
            }
        }
    }
}
