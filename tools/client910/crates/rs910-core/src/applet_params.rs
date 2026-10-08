//! The launcher's numbered applet parameters. The client reads them once at
//! startup; this port takes the same numbered keys from
//! `--applet-param KEY=VALUE` and defaults to the launcher's fixed values.
//! Keys the launcher marks dynamic (per-session website tokens such as 35
//! `siteSettings`) have no default. The values are process statics.
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The numbered applet parameter keys this port reads (revision data), plus
/// this client's own `SITE_DOMAIN`, which the launcher does not send.
mod key {
    pub const LABEL: u32 = 1;
    pub const AUTOMATED_FLAG_1: u32 = 3;
    pub const GAMEPACK: u32 = 6;
    pub const USER_FLOW_2: u32 = 9;
    pub const BUILD: u32 = 11;
    pub const CONTENT_PORT: u32 = 12;
    pub const HTTP_CONTENT_HOST: u32 = 14;
    pub const CURRENT_PLAYER_COUNTRY: u32 = 20;
    pub const CREATE_EMAIL: u32 = 21;
    pub const CONTENT_HOST: u32 = 23;
    pub const CLIENT_TYPE: u32 = 17;
    pub const PLAYER_IS_AFFILIATE: u32 = 24;
    pub const USER_FLOW_1: u32 = 25;
    pub const LANGUAGE: u32 = 26;
    pub const MODE_WHERE: u32 = 27;
    pub const CONTENT_PORT2: u32 = 30;
    pub const JAVASCRIPT: u32 = 31;
    pub const LOADING_BAR_COLOUR: u32 = 34;
    pub const SITE_SETTINGS: u32 = 35;
    pub const AUTOMATED_FLAG_2: u32 = 39;
    pub const CHROME: u32 = 42;
    pub const HTTP_CONTENT_PORT: u32 = 44;
    pub const MODE_GAME: u32 = 46;
    pub const ADDITIONAL_INFO: u32 = 51;
    pub const FROM_BILLING: u32 = 55;
    /// The domain of the website that `openurl_nologin` opens.
    pub const SITE_DOMAIN: u32 = 1000;
}

/// The website domain used when `SITE_DOMAIN` is not set: a neutral placeholder
/// for the operator to replace.
const DEFAULT_SITE_DOMAIN: &str = "example.com";

/// The launcher's fixed (non-host, non-dynamic) parameters this port reads.
const DEFAULTS: [(u32, &str); 6] = [
    (key::CURRENT_PLAYER_COUNTRY, "110"),
    (key::PLAYER_IS_AFFILIATE, "0"),
    (key::LANGUAGE, "0"),
    (key::MODE_WHERE, "0"),
    (key::MODE_GAME, "0"),
    (key::FROM_BILLING, "false"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppletParams {
    values: BTreeMap<u32, String>,
}

impl Default for AppletParams {
    fn default() -> Self {
        Self {
            values: DEFAULTS
                .iter()
                .map(|&(key, value)| (key, value.to_owned()))
                .collect(),
        }
    }
}

static PARAMS: OnceLock<AppletParams> = OnceLock::new();

/// Install the launcher parameters once at startup (startup runs once).
pub fn install(overrides: &[String]) -> crate::Result<()> {
    let params = AppletParams::parse(overrides)?;
    let _ = PARAMS.set(params);
    Ok(())
}

/// The installed parameters, or the launcher defaults.
pub fn get() -> &'static AppletParams {
    PARAMS.get_or_init(AppletParams::default)
}

impl AppletParams {
    pub fn parse(overrides: &[String]) -> crate::Result<Self> {
        let mut params = Self::default();
        for entry in overrides {
            let (key, value) = entry
                .split_once('=')
                .ok_or_else(|| crate::err!("--applet-param {entry}: expected KEY=VALUE"))?;
            let key: u32 = key
                .trim()
                .parse()
                .map_err(|_| crate::err!("--applet-param {entry}: key is not a number"))?;
            params.values.insert(key, value.to_owned());
        }
        // Startup parses these as integers / enum ids; an invalid launcher
        // value fails startup.
        params.mode_where()?;
        params.mode_game()?;
        params.language()?;
        for numeric_key in [
            key::AUTOMATED_FLAG_1,
            key::USER_FLOW_2,
            key::BUILD,
            key::CLIENT_TYPE,
            key::CURRENT_PLAYER_COUNTRY,
            key::PLAYER_IS_AFFILIATE,
            key::USER_FLOW_1,
            key::AUTOMATED_FLAG_2,
        ] {
            params.int(numeric_key)?;
        }
        Ok(params)
    }

    fn raw(&self, key: u32) -> Option<&str> {
        self.values.get(&key).map(String::as_str)
    }

    fn int(&self, key: u32) -> crate::Result<Option<i32>> {
        self.raw(key)
            .map(|v| {
                v.parse()
                    .map_err(|_| crate::err!("applet parameter {key}={v}: not a number"))
            })
            .transpose()
    }

    /// Applet parameter 27: the world mode by id; LOCAL becomes
    /// WTWIP and any non-office mode other than LIVE becomes LIVE. `None` is
    /// the unset mode.
    pub fn mode_where(&self) -> crate::Result<Option<i32>> {
        let Some(id) = self.int(key::MODE_WHERE)? else {
            return Ok(None);
        };
        // Mode ids; OFFICE members from their property lists.
        const OFFICE: [i32; 10] = [1, 2, 3, 5, 8, 9, 10, 11, 12, 13];
        const ALL: [i32; 13] = [0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13];
        crate::ensure!(
            ALL.contains(&id),
            "applet parameter 27={id}: unknown world mode"
        );
        Ok(Some(if id == 4 {
            3
        } else if !OFFICE.contains(&id) && id != 0 {
            0
        } else {
            id
        }))
    }

    /// Applet parameter 46: the game's title URL segment.
    pub fn mode_game(&self) -> crate::Result<Option<&'static str>> {
        let Some(id) = self.int(key::MODE_GAME)? else {
            return Ok(None);
        };
        const TITLE_URLS: [&str; 6] = [
            "runescape",
            "stellardawn",
            "game3",
            "game4",
            "game5",
            "oldscape",
        ];
        Ok(Some(
            usize::try_from(id)
                .ok()
                .and_then(|i| TITLE_URLS.get(i))
                .copied()
                .ok_or_else(|| crate::err!("applet parameter 46={id}: unknown game mode"))?,
        ))
    }

    /// The title of the launched game (applet parameter 46), which the
    /// window shows. An unknown game id falls back to the first game.
    pub fn game_title(&self) -> &'static str {
        const TITLES: [&str; 6] = [
            "RuneScape",
            "Stellar Dawn",
            "Game 3",
            "Game 4",
            "Game 5",
            "RuneScape 2007",
        ];
        usize::try_from(self.mode_game_id())
            .ok()
            .and_then(|i| TITLES.get(i))
            .copied()
            .unwrap_or(TITLES[0])
    }

    /// Applet parameter 26: the language id.
    pub fn language(&self) -> crate::Result<Option<i32>> {
        let Some(id) = self.int(key::LANGUAGE)? else {
            return Ok(None);
        };
        crate::ensure!(
            (0..=6).contains(&id),
            "applet parameter 26={id}: unknown Language"
        );
        Ok(Some(id))
    }

    /// Applet parameter 24, `playerIsAffiliate` (default 0).
    pub fn player_is_affiliate(&self) -> i32 {
        self.int(key::PLAYER_IS_AFFILIATE)
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    /// Applet parameters 9 and 25: the user-flow words, second then first
    /// (default 0).
    pub fn user_flow(&self) -> [i32; 2] {
        [
            self.word(key::USER_FLOW_2, 0),
            self.word(key::USER_FLOW_1, 0),
        ]
    }

    /// Applet parameters 3 and 39: the automated-test flag words (default 0).
    pub fn automated_test_flags(&self) -> [i32; 2] {
        [
            self.word(key::AUTOMATED_FLAG_1, 0),
            self.word(key::AUTOMATED_FLAG_2, 0),
        ]
    }

    /// Applet parameter 1: the launcher's label for this client (default empty).
    pub fn label(&self) -> String {
        self.raw(key::LABEL).unwrap_or_default().to_owned()
    }

    /// Applet parameter 51: the note an account-creation page passed along;
    /// one over a hundred characters is dropped.
    pub fn additional_info(&self) -> Option<String> {
        self.raw(key::ADDITIONAL_INFO)
            .filter(|note| note.encode_utf16().count() <= 100)
            .map(str::to_owned)
    }

    /// Applet parameter 31: the page has JavaScript (default false).
    pub fn javascript_enabled(&self) -> bool {
        self.flag(key::JAVASCRIPT)
    }

    /// Applet parameter 42: the browser is Chrome (default false).
    pub fn have_chrome(&self) -> bool {
        self.flag(key::CHROME)
    }

    /// Applet parameter 17: the client type (default 0).
    pub fn client_type(&self) -> i32 {
        self.word(key::CLIENT_TYPE, 0)
    }

    /// Applet parameter 11: the launcher's build word (-1 when absent).
    pub fn build_word(&self) -> i32 {
        self.word(key::BUILD, -1)
    }

    fn word(&self, key: u32, default: i32) -> i32 {
        self.int(key).ok().flatten().unwrap_or(default)
    }

    fn flag(&self, key: u32) -> bool {
        self.raw(key)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    /// Applet parameter 20, `currentPlayerCountry` (default 0).
    pub fn current_player_country(&self) -> i32 {
        self.int(key::CURRENT_PLAYER_COUNTRY)
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    /// Applet parameter 34: the
    /// loading-bar colour set, reset to 0 when out of range.
    pub fn loading_bar_colour(&self) -> usize {
        self.int(key::LOADING_BAR_COLOUR)
            .ok()
            .flatten()
            .filter(|v| (0..4).contains(v))
            .unwrap_or(0) as usize
    }

    /// Applet parameter 23: the content host.
    pub fn content_host(&self) -> Option<String> {
        self.raw(key::CONTENT_HOST).map(str::to_owned)
    }

    /// Applet parameter 12: the content port.
    pub fn content_port(&self) -> Option<u16> {
        self.raw(key::CONTENT_PORT)
            .and_then(|v| v.trim().parse().ok())
    }

    /// Applet parameter 30: the content secondary port (JS5).
    pub fn content_port2(&self) -> Option<u16> {
        self.raw(key::CONTENT_PORT2)
            .and_then(|v| v.trim().parse().ok())
    }

    /// Applet parameter 14: the HTTP content host.
    pub fn http_content_host(&self) -> Option<String> {
        self.raw(key::HTTP_CONTENT_HOST).map(str::to_owned)
    }

    /// Applet parameter 44: the HTTP content port.
    pub fn http_content_port(&self) -> Option<u16> {
        self.raw(key::HTTP_CONTENT_PORT)
            .and_then(|v| v.trim().parse().ok())
    }

    /// Applet parameter 6: the gamepack, sent in the JS5
    /// handshake.
    pub fn gamepack(&self) -> Option<String> {
        self.raw(key::GAMEPACK).map(str::to_owned)
    }

    /// Applet parameter 46 as a game id (the JS5 HTTP `/ms?m=` value).
    pub fn mode_game_id(&self) -> i32 {
        self.int(key::MODE_GAME).ok().flatten().unwrap_or(0)
    }

    /// Applet parameter 21, `createEmail` (default none).
    pub fn create_email(&self) -> Option<String> {
        self.raw(key::CREATE_EMAIL).map(str::to_owned)
    }

    /// Applet parameter 55, `fromBilling` (default false).
    pub fn is_from_billing(&self) -> bool {
        self.raw(key::FROM_BILLING).is_some_and(|v| v == "true")
    }

    /// Applet parameter 35, `siteSettings`, or "" when unset.
    pub fn site_settings(&self) -> String {
        self.raw(key::SITE_SETTINGS).unwrap_or_default().to_owned()
    }

    /// The website base domain (`SITE_DOMAIN`, default `example.com`); the host
    /// and game title segments come before it.
    pub fn site_domain(&self) -> &str {
        self.raw(key::SITE_DOMAIN)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(DEFAULT_SITE_DOMAIN)
    }

    /// The website URL for a site-settings token, built from the world mode.
    pub fn site_url(&self, site_settings: &str) -> crate::Result<String> {
        let mut host = "www".to_owned();
        match self.mode_where()? {
            Some(1) => host.push_str("-wtrc"),
            Some(2) => host.push_str("-wtqa"),
            Some(3) => host.push_str("-wtwip"),
            Some(5) => host.push_str("-wti"),
            Some(10) => host.push_str("-demo"),
            Some(4) => host = "local".into(),
            _ => {}
        }
        // The site settings are never null after startup.
        let settings = format!("/p={site_settings}");
        let game = self
            .mode_game()?
            .ok_or_else(|| crate::err!("applet parameter 46 (game mode) is unset"))?;
        let language = self
            .language()?
            .ok_or_else(|| crate::err!("applet parameter 26 (language) is unset"))?;
        Ok(format!(
            "http://{host}.{game}.{}/l={language}/a={}{settings}/",
            self.site_domain(),
            self.player_is_affiliate()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_url_per_world_mode() {
        let params = AppletParams::default();
        assert_eq!(
            params.site_url("").unwrap(),
            "http://www.runescape.example.com/l=0/a=0/p=/"
        );
        let params = AppletParams::parse(&[
            "27=2".into(),
            "46=5".into(),
            "26=1".into(),
            "24=7".into(),
            "1000=play.example.org".into(),
        ])
        .unwrap();
        assert_eq!(
            params.site_url("abc").unwrap(),
            "http://www-wtqa.oldscape.play.example.org/l=1/a=7/p=abc/"
        );
        // LOCAL becomes WTWIP; INTBETA (non-office) becomes LIVE.
        let local = AppletParams::parse(&["27=4".into()]).unwrap();
        assert_eq!(local.mode_where().unwrap(), Some(3));
        assert!(local.site_url("").unwrap().starts_with("http://www-wtwip."));
        let beta = AppletParams::parse(&["27=6".into()]).unwrap();
        assert_eq!(beta.mode_where().unwrap(), Some(0));
        assert!(AppletParams::parse(&["27=7".into()]).is_err());
        assert!(AppletParams::parse(&["46=x".into()]).is_err());
        assert!(AppletParams::parse(&["46".into()]).is_err());
        assert_eq!(params.current_player_country(), 110);
    }

    #[test]
    fn launcher_parameters_read_as_the_client_does() {
        let params = AppletParams::default();
        assert_eq!(params.user_flow(), [0, 0]);
        assert_eq!(params.automated_test_flags(), [0, 0]);
        assert_eq!(params.build_word(), -1, "an absent build word reads -1");
        assert_eq!(params.client_type(), 0);
        assert!(!params.javascript_enabled() && !params.have_chrome());
        assert_eq!(
            (params.label(), params.additional_info()),
            (String::new(), None)
        );
        let long_note = format!("51={}", "n".repeat(101));
        let params = AppletParams::parse(&[
            "9=5".into(),
            "25=3".into(),
            "3=7".into(),
            "39=9".into(),
            "11=1449949008".into(),
            "17=14561".into(),
            "31=TRUE".into(),
            "42=yes".into(),
            "1=abc".into(),
            "51=extra".into(),
        ])
        .unwrap();
        assert_eq!(params.user_flow(), [5, 3]);
        assert_eq!(params.automated_test_flags(), [7, 9]);
        assert_eq!(
            (params.build_word(), params.client_type()),
            (1_449_949_008, 14561)
        );
        assert!(params.javascript_enabled(), "any case of true");
        assert!(!params.have_chrome(), "only true is true");
        assert_eq!(params.label(), "abc");
        assert_eq!(params.additional_info().as_deref(), Some("extra"));
        let long = AppletParams::parse(&[long_note]).unwrap();
        assert_eq!(
            long.additional_info(),
            None,
            "over a hundred characters is dropped"
        );
        assert!(AppletParams::parse(&["9=x".into()]).is_err());
    }
}
