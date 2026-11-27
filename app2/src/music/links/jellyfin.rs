use std::sync::atomic::AtomicBool;

use url::{Url, form_urlencoded};

use crate::{
    music::ObjectRef,
    providers::{ProviderId, catalog},
    storage::Store,
};

pub(in crate::music) struct DetailsLink {
    web_root: Url,
    item_id: String,
}

impl DetailsLink {
    pub(in crate::music) fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.contains('\\') || value.bytes().any(|byte| byte.is_ascii_control()) {
            return None;
        }
        let (web_address, fragment) = value.split_once('#')?;
        let (authority, raw_path) = web_address.split_once("://")?.1.split_once('/')?;
        // The URL parser discards an empty user-information marker.
        if authority.is_empty() || authority.contains('@') {
            return None;
        }
        let mut web_root = Url::parse(web_address).ok()?;
        if !matches!(web_root.scheme(), "http" | "https")
            || web_root.host_str().is_none()
            || web_root.query().is_some()
            || web_root.path().strip_prefix('/')? != raw_path
        {
            return None;
        }
        let query = if web_root.path().ends_with("/web/") {
            fragment.strip_prefix("/details?")?
        } else if web_root.path().ends_with("/web/index.html") {
            web_root = web_root.join("./").ok()?;
            fragment.strip_prefix("!/details?")?
        } else {
            return None;
        };
        let mut item_id = None;
        let mut has_server_id = false;
        let mut has_context = false;
        for (key, value) in form_urlencoded::parse(query.as_bytes()) {
            match key.as_ref() {
                "id" if item_id.is_none() && catalog::valid_jellyfin_item_id(&value) => {
                    item_id = Some(value.into_owned());
                }
                "serverId" if !has_server_id && !value.is_empty() => {
                    has_server_id = true;
                }
                "context" if !has_context && !value.is_empty() => {
                    has_context = true;
                }
                _ => return None,
            }
        }
        Some(Self {
            web_root,
            item_id: item_id?,
        })
    }

    pub(in crate::music) fn resolve(
        self,
        store: &Store,
        cancelled: &AtomicBool,
    ) -> Result<(ObjectRef, String), String> {
        let account = store
            .preferred_account(ProviderId::Jellyfin)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Подключите сервер Jellyfin, чтобы открыть ссылку.".to_owned())?;
        let credentials = store
            .credentials(&account)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Подтвердите вход в Jellyfin заново.".to_owned())?;
        let object =
            catalog::resolve_jellyfin_link(&credentials, &self.web_root, &self.item_id, cancelled)
                .map_err(|error| error.to_string())?;
        if store
            .preferred_account(ProviderId::Jellyfin)
            .map_err(|error| error.to_string())?
            .as_ref()
            != Some(&account)
        {
            return Err("Подключение Jellyfin изменилось во время открытия ссылки.".into());
        }

        let mut canonical = self.web_root;
        canonical.set_fragment(Some(&format!("/details?id={}", object.id)));
        Ok((object, canonical.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "abcdef0123456789abcdef0123456789";

    #[test]
    fn reads_jellyfin_details_with_port_and_reverse_proxy_prefix() {
        let link = DetailsLink::parse(&format!(
            "http://localhost:8096/jellyfin/web/#/details?id={ID}&context=music&serverId={ID}"
        ))
        .unwrap();
        assert_eq!(
            link.web_root.as_str(),
            "http://localhost:8096/jellyfin/web/"
        );
        assert_eq!(link.item_id, ID);
    }

    #[test]
    fn normalizes_legacy_jellyfin_details_to_the_same_web_root() {
        let link = DetailsLink::parse(&format!(
            "https://jellyfin.example/custom%20prefix/web/index.html#!/details?id={ID}&serverId={ID}"
        ))
        .unwrap();
        assert_eq!(
            link.web_root.as_str(),
            "https://jellyfin.example/custom%20prefix/web/"
        );
        assert_eq!(link.item_id, ID);
    }

    #[test]
    fn refuses_noncanonical_details_and_ambiguous_credentials() {
        for address in [
            format!("http://@localhost:8096/web/#/details?id={ID}"),
            format!("http://localhost:8096/web/#!/details?id={ID}"),
            format!("http://localhost:8096/web/index.html#/details?id={ID}"),
            format!("http://localhost:8096/web/?api_key=private#/details?id={ID}"),
            format!("http://localhost:8096/web/#/details?id={ID}&api_key=private"),
            format!("http://localhost:8096/web/#/details?id={ID}&id={ID}"),
            format!("http://localhost:8096/other/../web/#/details?id={ID}"),
            "http://localhost:8096/web/#/details?id=other".into(),
        ] {
            assert!(DetailsLink::parse(&address).is_none(), "{address}");
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "uses an isolated Jellyfin test server and an empty Mediadeka profile"]
    fn live_jellyfin_details_open_through_music_search() {
        use std::{env, sync::Mutex};

        use crate::{
            music::{ObjectKind, PublicInput, SearchInput, resolve_public_input},
            providers::{LoginInput, sign_in},
        };

        let fixture = |key| {
            env::var(format!("MEDIADEKA_TEST_JELLYFIN_{key}")).expect("isolated fixture variable")
        };
        let server = fixture("URL");
        let endpoint = Url::parse(&server).unwrap();
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        assert!(endpoint.port().is_some());
        let username = fixture("USER");
        assert_eq!(username, "mediadeka-fixture");
        assert!(env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
        let profile = Store::profile_directory().unwrap();
        assert!(!profile.join("mediadeka.sqlite3").exists());
        let store = Store::open(profile).unwrap();
        let provider = ProviderId::Jellyfin;
        let cancelled = AtomicBool::new(false);
        let session = sign_in(
            provider,
            LoginInput::JellyfinPassword {
                server: server.clone(),
                username,
                password: fixture("PASSWORD"),
            },
            &cancelled,
            |_| {},
        )
        .expect("sign in to the fixture");
        assert!(store.use_account(&session, || Some(()), || {}).unwrap());
        struct FixtureAccount<'a>(&'a Store, &'a str);
        impl Drop for FixtureAccount<'_> {
            fn drop(&mut self) {
                let _ = self
                    .0
                    .disconnect_account(ProviderId::Jellyfin, self.1, || Some(()));
            }
        }
        let _account = FixtureAccount(&store, &session.profile.id);

        let id = fixture("ITEM");
        let expected = ObjectRef {
            provider,
            kind: ObjectKind::Track,
            id: id.clone(),
        };
        let canonical = format!("{server}/web/#/details?id={id}");
        for address in [
            format!("{canonical}&context=music"),
            format!("{server}/web/index.html#!/details?id={id}"),
        ] {
            let Some(SearchInput::Url(url)) = SearchInput::new(ObjectKind::Track, &address) else {
                panic!("expected URL search input");
            };
            let results = Mutex::new(Vec::new());
            resolve_public_input(&url, &cancelled, |result| {
                results.lock().unwrap().push(result);
            })
            .expect("resolve the fixture from a link");
            let results = results.into_inner().unwrap();
            let [PublicInput::Address(object, resolved_url)] = results.as_slice() else {
                panic!("expected one address result");
            };
            assert_eq!(object, &expected);
            assert_eq!(resolved_url, &canonical);
        }

        let other_server = canonical.replacen("127.0.0.1", "localhost", 1);
        assert_eq!(
            resolve_public_input(&other_server, &cancelled, |_| panic!("unexpected result")),
            Err("Ссылка ведёт на другой сервер Jellyfin.".into())
        );
        println!(
            "Jellyfin search opened both forms as the same recording and rejected another server"
        );
    }
}
