use discord_rich_presence::activity::{
    ActivityType, Button as ActButton, StatusDisplayType as DiscordIpcStatusDisplayType,
};
use discord_rich_presence::{
    activity::{Activity, Assets, Timestamps},
    DiscordIpc, DiscordIpcClient,
};
pub use error::JfError;
pub use jellyfin::{Button, MediaType};
use jellyfin::{ExternalUrl, NowPlayingItem, PlayTime, RawSession, Session, VirtualFolder};
use log::{debug, warn};
use reqwest::header::{HeaderMap, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::SystemTime;
use url::Url;

mod error;
mod external;
mod jellyfin;
#[cfg(test)]
mod tests;

pub(crate) type JfResult<T> = Result<T, Box<dyn std::error::Error>>;

pub const VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

/// Client used to interact with jellyfin and discord
pub struct Client {
    discord_ipc_client: DiscordIpcClient,
    url: Url,
    usernames: Vec<String>,
    reqwest: reqwest::blocking::Client,
    session: Option<Session>,
    buttons: Option<Vec<Button>>,
    music_display_options: DisplayOptions,
    movies_display_options: DisplayOptions,
    episodes_display_options: DisplayOptions,
    blacklist: Blacklist,
    show_paused: bool,
    show_images: bool,
    imgur_options: ImgurOptions,
    litterbox_options: LitterboxOptions,
    process_images: bool,
    large_image_text: String,
}

impl Client {
    /// Calls the `ClientBuilder::new()` function
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Connects to the discord socket
    pub fn connect(&mut self) -> JfResult<()> {
        self.discord_ipc_client.connect()?;
        Ok(())
    }

    /// Reconnects to the discord socket
    pub fn reconnect(&mut self) -> JfResult<()> {
        self.discord_ipc_client.reconnect()?;
        Ok(())
    }

    /// Clears current activity on discord if anything is being displayed
    ///
    /// # Example
    /// ```no_run
    /// use jellyfin_rpc::Client;
    ///
    /// let mut builder = Client::builder();
    /// builder.api_key("abcd1234")
    ///     .url("https://jellyfin.example.com")
    ///     .username("user");
    ///
    /// let mut client = builder.build().unwrap();
    ///
    /// client.connect().unwrap();
    ///
    /// client.set_activity().unwrap();
    ///
    /// client.clear_activity().unwrap();
    /// ```
    pub fn clear_activity(&mut self) -> JfResult<()> {
        self.discord_ipc_client.clear_activity()?;
        Ok(())
    }

    /// Gathers information from jellyfin about what is being played and displays it according to the options supplied to the builder.
    ///
    /// # Example
    /// ```no_run
    /// use jellyfin_rpc::Client;
    ///
    /// let mut builder = Client::builder();
    /// builder.api_key("abcd1234")
    ///     .url("https://jellyfin.example.com")
    ///     .username("user");
    ///
    /// let mut client = builder.build().unwrap();
    ///
    /// client.connect().unwrap();
    ///
    /// client.set_activity().unwrap();
    /// ```
    pub fn set_activity(&mut self) -> JfResult<String> {
        self.get_session()?;

        // Make sure the blacklist cache is loaded/valid
        match &self.blacklist.libraries {
            BlacklistedLibraries::Uninitialized => {
                self.reload_blacklist();
            }
            BlacklistedLibraries::Initialized(_, init_time) => {
                if SystemTime::now()
                    .duration_since(*init_time)
                    .map(|passed| passed.as_secs() > 3600)
                    .unwrap_or(false)
                {
                    debug!("reloading blacklist after cache expiration");
                    self.reload_blacklist();
                }
            }
        }

        if let Some(session) = &self.session {
            if session.now_playing_item.media_type == MediaType::None {
                return Err(Box::new(JfError::UnrecognizedMediaType));
            }

            if self.check_blacklist()? {
                return Err(Box::new(JfError::ContentBlacklist));
            }

            let mut activity = Activity::new();

            let mut image_url = Url::from_str("https://i.imgur.com/oX6vcds.png")?;

            if session.now_playing_item.media_type == MediaType::LiveTv {
                image_url = Url::from_str("https://i.imgur.com/XxdHOqm.png")?;
            } else if self.imgur_options.enabled && self.show_images {
                if let Ok(imgur_url) = external::imgur::get_image(self) {
                    image_url = imgur_url;
                } else {
                    debug!("imgur::get_image() didnt return an image, using default..")
                }
            } else if self.litterbox_options.enabled && self.show_images {
                if let Ok(litterbox_url) = external::litterbox::get_image(self) {
                    image_url = litterbox_url;
                } else {
                    debug!("litterbox::get_image() didn't return an image, using default..")
                }
            } else if self.show_images {
                if let Ok(iu) = self.get_image() {
                    image_url = iu;
                } else {
                    debug!("self.get_image() didnt return an image, using default..")
                }
            }

            let mut assets = Assets::new().large_image(image_url.as_str());

            if !self.large_image_text.is_empty() {
                assets = assets.large_text(&self.large_image_text);
            }

            let mut timestamps = Timestamps::new();

            match session.get_time()? {
                PlayTime::Some(start, end) => timestamps = timestamps.start(start).end(end),
                PlayTime::None => (),
                PlayTime::Paused if self.show_paused => {
                    assets = assets
                        .small_image("https://i.imgur.com/wlHSvYy.png")
                        .small_text("Paused");
                }
                PlayTime::Paused => return Ok(String::new()),
            }

            let buttons: Vec<Button>;

            if let Some(b) = self.get_buttons() {
                // This gets around the value being dropped immediately at the end of this if statement
                buttons = b;
                activity = activity.buttons(
                    buttons
                        .iter()
                        .map(|b| ActButton::new(&b.name, &b.url))
                        .collect(),
                );
            }

            let mut state = self.get_state();

            if state.len() > 128 {
                state = state.chars().take(128).collect();
            } else if state.len() < 3 {
                // Add three zero width joiners due to discord requiring a minimum length of 3 chars in statuses
                state += "‎‎‎";
            }

            let mut details = self.get_details();

            if details.len() > 128 {
                details = details.chars().take(128).collect();
            } else if details.len() < 3 {
                // add three (3) zero width joiners
                details += "‎‎‎";
            }

            let mut image_text = self.get_image_text();

            if image_text.is_empty() {
                image_text = format!("Jellyfin-RPC v{}", VERSION.unwrap_or("UNKNOWN"));
            }

            if image_text.len() > 128 {
                image_text = image_text.chars().take(128).collect();
            } else if image_text.len() < 3 {
                // add three zero width joiners
                image_text += "‎‎‎";
            }

            assets = assets.large_text(image_text.as_str());

            match session.now_playing_item.media_type {
                MediaType::Book => (),
                MediaType::Music | MediaType::AudioBook => {
                    activity = activity.activity_type(ActivityType::Listening)
                }
                _ => activity = activity.activity_type(ActivityType::Watching),
            }

            let status_display_type = self.get_status_display_type();

            activity = activity
                .timestamps(timestamps)
                .assets(assets)
                .details(&details)
                .state(&state)
                .status_display_type(status_display_type.into());

            self.discord_ipc_client.set_activity(activity)?;

            return Ok(format!("{} | {}", details, state));
        }
        Ok(String::new())
    }

    fn get_session(&mut self) -> JfResult<()> {
        let sessions: Vec<RawSession> = self
            .reqwest
            .get(self.url.join("Sessions")?)
            .send()?
            .json()?;

        debug!("Found {} sessions", sessions.len());

        for session in sessions {
            debug!("Session username is {:?}", session.user_name);
            if let Some(username) = session.user_name.as_ref() {
                if self
                    .usernames
                    .iter()
                    .all(|u| username.to_lowercase() != u.to_lowercase())
                {
                    continue;
                }

                if session.now_playing_item.is_none() {
                    continue;
                }
                debug!("NowPlayingItem exists");

                if session.play_state.is_none() {
                    continue;
                }
                debug!("PlayState exists");

                let session = session.build();

                if session
                    .now_playing_item
                    .extra_type
                    .as_ref()
                    .is_some_and(|et| et == "ThemeSong")
                {
                    debug!("Session is playing a theme song, continuing loop");
                    continue;
                }

                self.session = Some(session);
                return Ok(());
            }
        }
        self.session = None;
        Ok(())
    }

    fn get_buttons(&self) -> Option<Vec<Button>> {
        let session = self.session.as_ref()?;

        let mut activity_buttons: Vec<Button> = Vec::new();

        if let (Some(ext_urls), Some(buttons)) = (
            &session.now_playing_item.external_urls,
            self.buttons.as_ref(),
        ) {
            let ext_urls: Vec<&ExternalUrl> = ext_urls
                .iter()
                .filter(|eu| {
                    !eu.url.starts_with("http://localhost")
                        && !eu.url.starts_with("https://localhost")
                })
                .collect();
            let mut i = 0;
            for button in buttons {
                if activity_buttons.len() == 2 {
                    break;
                }

                if button.is_dynamic() {
                    if ext_urls.len() > i {
                        activity_buttons.push(Button::new(
                            ext_urls[i].name.clone(),
                            ext_urls[i].url.clone(),
                        ));
                        i += 1;
                    }
                } else {
                    activity_buttons.push(button.clone())
                }
            }
            return Some(activity_buttons);
        } else if let Some(buttons) = self.buttons.as_ref() {
            for button in buttons {
                if activity_buttons.len() == 2 {
                    break;
                }

                if !button.is_dynamic() {
                    activity_buttons.push(button.clone())
                }
            }
            return Some(activity_buttons);
        } else if let Some(ext_urls) = &session.now_playing_item.external_urls {
            let ext_urls: Vec<&ExternalUrl> = ext_urls
                .iter()
                .filter(|eu| {
                    !eu.url.starts_with("http://localhost")
                        && !eu.url.starts_with("https://localhost")
                })
                .collect();
            for ext_url in ext_urls {
                if activity_buttons.len() == 2 {
                    break;
                }

                activity_buttons.push(Button::new(ext_url.name.clone(), ext_url.url.clone()))
            }
            return Some(activity_buttons);
        }
        None
    }

    fn get_image(&self) -> JfResult<Url> {
        let session = self.session.as_ref().unwrap();

        let ids: Vec<&str> = if matches!(session.now_playing_item.media_type, MediaType::Music) {
            vec![&session.now_playing_item.id, &session.item_id]
        } else {
            vec![&session.item_id]
        };

        for id in ids {
            let path = "Items/".to_string() + id + "/Images/Primary";

            let image_url = self.url.join(&path)?;

            if !self
                .reqwest
                .get(image_url.as_ref())
                .send()?
                .text()?
                .contains("does not have an image of type Primary")
            {
                return Ok(image_url);
            }
        }

        Err(Box::new(JfError::NoImage))
    }

    fn sanitize_display_format(input: &str) -> String {
        // Remove unnecessary spaces
        let mut result = input.split_whitespace().collect::<Vec<&str>>().join(" ");

        // Remove duplicated separators
        while result.contains("{sep}{sep}") || result.contains("{sep} {sep}") {
            result = result.replace("{sep}{sep}", "{sep}");
            result = result.replace("{sep} {sep}", "{sep}");
        }

        // Remove unnecessary separators
        while result.starts_with("{sep}") {
            result = result
                .drain(5..)
                .collect::<String>()
                .trim_start()
                .to_string();
        }

        while result.ends_with("{sep}") {
            result = result
                .drain(..result.len() - 5)
                .collect::<String>()
                .trim_end()
                .to_string();
        }

        result
    }

    fn parse_music_display(&self, input: &str) -> String {
        let mut result = input.trim().to_string();
        let session = self.session.as_ref().unwrap();

        let separator = &self.music_display_options.separator;
        let track = session.now_playing_item.name.as_ref();
        let artists = session.format_artists();
        let genres = session
            .now_playing_item
            .genres
            .as_ref()
            .unwrap_or(&vec!["".to_string()])
            .join(", ");
        let year = session
            .now_playing_item
            .production_year
            .map(|y| y.to_string())
            .unwrap_or_default();
        let album = session
            .now_playing_item
            .album
            .as_ref()
            .unwrap_or(&"".to_string())
            .clone();

        result = result
            .replace("{track}", track)
            .replace("{album}", &album)
            .replace("{artists}", &artists)
            .replace("{genres}", &genres)
            .replace("{year}", &year)
            .replace("{version}", VERSION.unwrap_or("UNKNOWN"));

        Self::sanitize_display_format(&result).replace("{sep}", separator)
    }

    fn parse_movies_display(&self, input: &str) -> String {
        let mut result = input.trim().to_string();
        let session = self.session.as_ref().unwrap();

        let separator = &self.movies_display_options.separator;
        let title = session.now_playing_item.name.as_ref();
        let original_title = session
            .now_playing_item
            .original_title
            .as_ref()
            .unwrap_or(&"".to_string())
            .clone();
        let genres = &session
            .now_playing_item
            .genres
            .as_ref()
            .unwrap_or(&vec!["".to_string()])
            .join(", ");
        let year = session
            .now_playing_item
            .production_year
            .map(|y| y.to_string())
            .unwrap_or_default();
        let critic_score = &session
            .now_playing_item
            .critic_rating
            .map(|s| format!("🍅 {}/100", s))
            .unwrap_or_default();
        let community_score = &session
            .now_playing_item
            .community_rating
            .map(|s| format!("⭐ {:.1}/10", s))
            .unwrap_or_default();

        result = result
            .replace("{title}", title)
            .replace("{original-title}", &original_title)
            .replace("{genres}", &genres)
            .replace("{year}", &year)
            .replace("{critic-score}", critic_score)
            .replace("{community-score}", community_score)
            .replace("{version}", VERSION.unwrap_or("UNKNOWN"));

        Self::sanitize_display_format(&result).replace("{sep}", separator)
    }

    fn parse_episodes_display(&self, input: &str) -> String {
        let mut result = input.trim().to_string();
        let session = self.session.as_ref().unwrap();

        let separator = &self.episodes_display_options.separator;
        let show_title = session
            .now_playing_item
            .series_name
            .as_ref()
            .unwrap_or(&"".to_string())
            .clone();
        let episode_title = session.now_playing_item.name.as_ref();
        let original_title = session
            .now_playing_item
            .original_title
            .as_ref()
            .unwrap_or(&"".to_string())
            .clone();
        let season = session.now_playing_item.parent_index_number.unwrap_or(0);
        let year = session
            .now_playing_item
            .production_year
            .map(|y| y.to_string())
            .unwrap_or_default();
        let genres = session
            .now_playing_item
            .genres
            .as_ref()
            .unwrap_or(&vec!["".to_string()])
            .join(", ");
        let studio = session
            .now_playing_item
            .series_studio
            .as_ref()
            .unwrap_or(&"".to_string())
            .clone();

        // One episode on Jellyfin can span across multiple actual episodes
        // For example E01-03 is 3 episodes in one media file
        let episode_range = (
            session.now_playing_item.index_number.unwrap_or(0),
            session.now_playing_item.index_number_end,
        );
        result = result
            .replace("{show-title}", &show_title)
            .replace("{title}", episode_title)
            .replace("{original-title}", &original_title)
            .replace(
                "{episode}",
                &match episode_range {
                    (first, Some(last)) => format!("{}-{}", first, last),
                    (episode, None) => format!("{}", episode),
                },
            )
            .replace(
                "{episode-padded}",
                &match episode_range {
                    (first, Some(last)) => format!("{:02}-{:02}", first, last),
                    (episode, None) => format!("{:02}", episode),
                },
            )
            .replace("{season}", &season.to_string())
            .replace("{season-padded}", &format!("{:02}", season))
            .replace("{year}", &year)
            .replace("{genres}", &genres)
            .replace("{studio}", &studio)
            .replace("{version}", VERSION.unwrap_or("UNKNOWN"));

        Self::sanitize_display_format(&result).replace("{sep}", separator)
    }

    fn get_details(&self) -> String {
        let session = self.session.as_ref().unwrap();

        match session.now_playing_item.media_type {
            MediaType::Music => {
                let display_details_format = &self
                    .music_display_options
                    .display
                    .details_text
                    .as_ref()
                    .unwrap();
                self.parse_music_display(
                    display_details_format
                        .replace("{__default}", "{track}")
                        .as_str(),
                )
            }
            MediaType::Movie => {
                let display_details_format = &self
                    .movies_display_options
                    .display
                    .details_text
                    .as_ref()
                    .unwrap();
                self.parse_movies_display(
                    display_details_format
                        .replace("{__default}", "{title}")
                        .as_str(),
                )
            }
            MediaType::Episode => {
                let display_details_format = &self
                    .episodes_display_options
                    .display
                    .details_text
                    .as_ref()
                    .unwrap();
                self.parse_episodes_display(
                    display_details_format
                        .replace("{__default}", "{show-title}")
                        .as_str(),
                )
            }
            MediaType::AudioBook => session
                .now_playing_item
                .album
                .as_ref()
                .map(|a| a.to_string())
                .unwrap_or_else(|| session.now_playing_item.name.to_string()),
            _ => session.now_playing_item.name.to_string(),
        }
    }

    fn get_state(&self) -> String {
        let session = self.session.as_ref().unwrap();

        match session.now_playing_item.media_type {
            MediaType::Episode => {
                let display_state_format = &self
                    .episodes_display_options
                    .display
                    .state_text
                    .as_ref()
                    .unwrap();
                self.parse_episodes_display(
                    display_state_format.replace("{__default}", "").as_str(),
                )
            }
            MediaType::LiveTv => "Live TV".to_string(),
            MediaType::Music => {
                let display_state_format = &self
                    .music_display_options
                    .display
                    .state_text
                    .as_ref()
                    .unwrap();
                self.parse_music_display(
                    display_state_format
                        .replace("{__default}", "By {artists} {sep} ")
                        .as_str(),
                )
            }
            MediaType::Book => {
                let mut state = String::new();

                if let Some(position_ticks) = session.play_state.position_ticks {
                    let ticks_to_pages = 10000;

                    let page = position_ticks / ticks_to_pages;

                    state += &format!("Reading page {}", page);
                }

                state
            }
            MediaType::AudioBook => {
                let mut state = String::new();

                let artists = session.format_artists();

                let genres = session
                    .now_playing_item
                    .genres
                    .as_ref()
                    .unwrap_or(&vec!["".to_string()])
                    .join(", ");

                if !artists.is_empty() {
                    state += &format!("By {}", artists)
                }

                if !state.is_empty() && !genres.is_empty() {
                    state += " - "
                }

                state += &genres;

                state
            }
            MediaType::Movie => {
                let display_state_format = &self
                    .movies_display_options
                    .display
                    .state_text
                    .as_ref()
                    .unwrap();
                self.parse_movies_display(display_state_format.replace("{__default}", "").as_str())
            }
            _ => session
                .now_playing_item
                .genres
                .as_ref()
                .unwrap_or(&vec!["".to_string()])
                .join(", "),
        }
    }

    fn get_status_display_type(&self) -> StatusType {
        let session = self.session.as_ref().unwrap();
        match session.now_playing_item.media_type {
            MediaType::Episode => self.episodes_display_options.status_display_type.clone(),
            MediaType::Movie => self.movies_display_options.status_display_type.clone(),
            MediaType::Music => self.music_display_options.status_display_type.clone(),
            _ => Default::default(),
        }
    }

    fn get_image_text(&self) -> String {
        let session = self.session.as_ref().unwrap();

        match session.now_playing_item.media_type {
            MediaType::Music => {
                let display_image_format = &self
                    .music_display_options
                    .display
                    .image_text
                    .as_ref()
                    .unwrap();
                self.parse_music_display(display_image_format)
            }
            MediaType::Movie => {
                let display_image_format = &self
                    .movies_display_options
                    .display
                    .image_text
                    .as_ref()
                    .unwrap();
                self.parse_movies_display(display_image_format)
            }
            MediaType::Episode => {
                let display_image_format = &self
                    .episodes_display_options
                    .display
                    .image_text
                    .as_ref()
                    .unwrap();
                self.parse_episodes_display(display_image_format)
            }
            _ => "".to_string(),
        }
    }

    fn check_blacklist(&self) -> JfResult<bool> {
        let session = self.session.as_ref().unwrap();

        if self
            .blacklist
            .media_types
            .iter()
            .any(|m| m == &session.now_playing_item.media_type)
        {
            return Ok(true);
        }

        if self.blacklist.check_item(&session.now_playing_item) {
            return Ok(true);
        }

        Ok(false)
    }

    /// Fetch the virtual folder list and filter out the blacklisted libraries
    fn fetch_blacklist(&self) -> JfResult<Vec<VirtualFolder>> {
        let virtual_folders: Vec<VirtualFolder> = self
            .reqwest
            .get(self.url.join("Library/VirtualFolders")?)
            .send()?
            .json()?;

        Ok(virtual_folders
            .into_iter()
            .filter(|library_folder| {
                self.blacklist
                    .libraries_names
                    .contains(library_folder.name.as_ref().unwrap_or(&String::new()))
            })
            .collect())
    }

    /// Reload the library list from Jellyfin and filter out the user-provided blacklisted libraries
    fn reload_blacklist(&mut self) {
        self.blacklist.libraries = match self.fetch_blacklist() {
            Ok(blacklist) => BlacklistedLibraries::Initialized(blacklist, SystemTime::now()),
            Err(err) => {
                warn!("Failed to intialize blacklist: {}", err);
                BlacklistedLibraries::Uninitialized
            }
        }
    }
}

pub struct EpisodeDisplayOptions {
    pub divider: bool,
    pub prefix: bool,
    pub simple: bool,
}

struct DisplayOptions {
    separator: String,
    display: DisplayFormat,
    status_display_type: StatusType,
}

/// Represents the formatting details for `Display`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct DisplayFormat {
    /// First line of the activity.
    pub details_text: Option<String>,
    /// Second line of the activity.
    pub state_text: Option<String>,
    /// Third line / large image text of the activity.
    pub image_text: Option<String>,
}

/// Converts legacy `Vec<String>` to `DisplayFormat`
impl From<Vec<String>> for DisplayFormat {
    fn from(items: Vec<String>) -> Self {
        let details_text = "{__default}".to_string();
        let image_text = "Jellyfin-RPC v{version}".to_string();
        let mut state_text = "{__default}".to_string();

        let items_joined = items
            .iter()
            .map(|i| format!("{{{}}}", i.trim()))
            .collect::<Vec<String>>()
            .join(" {sep} ");

        if !items_joined.is_empty() {
            state_text += &items_joined;
        }

        DisplayFormat {
            details_text: Some(details_text),
            state_text: Some(state_text),
            image_text: Some(image_text),
        }
    }
}

/// Reuses `DisplayFormat::from(Vec<String>)`
impl From<String> for DisplayFormat {
    fn from(item: String) -> Self {
        let data: Vec<String> = item.split(',').map(|d| d.to_string()).collect();
        DisplayFormat::from(data)
    }
}

/// Converts `EpisodeDisplayOptions` to `DisplayFormat`
impl From<EpisodeDisplayOptions> for DisplayFormat {
    fn from(value: EpisodeDisplayOptions) -> Self {
        let details_text = "{show-title}".to_string();
        let state_text = {
            let (season_tag, episode_tag) = if value.prefix {
                (
                    "S{season-padded}".to_string(),
                    "E{episode-padded}".to_string(),
                )
            } else {
                ("S{season}".to_string(), "E{episode}".to_string())
            };

            let divider = if value.divider { " - " } else { "" };

            if value.simple {
                format!("{}{}{}", season_tag, divider, episode_tag)
            } else {
                format!("{}{}{} {}", season_tag, divider, episode_tag, "{title}")
            }
        };
        let image_text = "Jellyfin-RPC v{version}".to_string();

        DisplayFormat {
            details_text: Some(details_text),
            state_text: Some(state_text),
            image_text: Some(image_text),
        }
    }
}

#[derive(Clone, Debug)]
pub enum StatusType {
    Name,
    State,
    Details,
}
impl Default for StatusType {
    fn default() -> Self {
        Self::Name
    }
}

impl From<DiscordIpcStatusDisplayType> for StatusType {
    fn from(x: DiscordIpcStatusDisplayType) -> Self {
        use DiscordIpcStatusDisplayType as T;
        match x {
            T::Name => Self::Name,
            T::State => Self::State,
            T::Details => Self::Details,
        }
    }
}

impl Into<DiscordIpcStatusDisplayType> for StatusType {
    fn into(self) -> DiscordIpcStatusDisplayType {
        use DiscordIpcStatusDisplayType as T;
        match self {
            Self::Name => T::Name,
            Self::State => T::State,
            Self::Details => T::Details,
        }
    }
}

#[derive(Debug)]
pub struct StatusTypeFromStringError;

impl TryFrom<String> for StatusType {
    type Error = StatusTypeFromStringError;
    fn try_from(x: String) -> Result<Self, Self::Error> {
        match x.as_ref() {
            "name" => Ok(Self::Name),
            "state" => Ok(Self::State),
            "details" => Ok(Self::Details),
            _ => Err(StatusTypeFromStringError),
        }
    }
}

struct Blacklist {
    media_types: Vec<MediaType>,
    libraries_names: Vec<String>,
    libraries: BlacklistedLibraries,
}

enum BlacklistedLibraries {
    Uninitialized,
    Initialized(Vec<VirtualFolder>, SystemTime),
}

impl Blacklist {
    /// Check whether a [NowPlayingItem] is in a blacklisted library
    fn check_item(&self, playing_item: &NowPlayingItem) -> bool {
        debug!("Checking if an item is blacklisted: {}", playing_item.name);
        self.check_path(playing_item.path.as_ref().unwrap_or(&String::new()))
    }

    /// Check whether a path is in a blacklisted library
    fn check_path(&self, item_path: &str) -> bool {
        match &self.libraries {
            BlacklistedLibraries::Initialized(libraries, _) => {
                debug!("Checking path: {}", item_path);
                libraries.iter().any(|blacklisted_mf| {
                    blacklisted_mf.locations.iter().any(|physical_folder| {
                        debug!("BL path: {}", physical_folder);
                        item_path.starts_with(physical_folder)
                    })
                })
            }
            BlacklistedLibraries::Uninitialized => false,
        }
    }
}

struct ImgurOptions {
    enabled: bool,
    client_id: String,
    urls_location: String,
}

struct LitterboxOptions {
    enabled: bool,
    urls_location: String
}

/// Used to build a new Client
#[derive(Default)]
pub struct ClientBuilder {
    url: String,
    client_id: String,
    api_key: String,
    self_signed: bool,
    usernames: Vec<String>,
    buttons: Option<Vec<Button>>,
    episode_divider: bool,
    episode_prefix: bool,
    episode_simple: bool,
    music_separator: String,
    music_display: DisplayFormat,
    music_status_display_type: StatusType,
    movies_separator: String,
    movies_display: DisplayFormat,
    movies_status_display_type: StatusType,
    episodes_separator: String,
    episodes_display: DisplayFormat,
    episodes_status_display_type: StatusType,
    blacklist_media_types: Vec<MediaType>,
    blacklist_libraries: Vec<String>,
    show_paused: bool,
    show_images: bool,
    use_imgur: bool,
    imgur_client_id: String,
    imgur_urls_file_location: String,
    use_litterbox: bool,
    litterbox_urls_file_location: String,
    large_image_text: String,
    process_images: bool,
}

impl ClientBuilder {
    /// Returns a ClientBuilder with some default options set
    pub fn new() -> Self {
        Self {
            client_id: "1053747938519679018".to_string(),
            music_separator: "-".to_string(),
            music_display: DisplayFormat::from(vec!["genres".to_string()]),
            movies_separator: "-".to_string(),
            movies_display: DisplayFormat::from(vec!["genres".to_string()]),
            episodes_separator: "-".to_string(),
            episodes_display: DisplayFormat::from(EpisodeDisplayOptions {
                divider: true,
                prefix: true,
                simple: false,
            }),
            show_paused: true,
            process_images: true,
            ..Default::default()
        }
    }

    /// Jellyfin URL to be used by the client.
    ///
    /// Has no default.
    pub fn url<T: Into<String>>(&mut self, url: T) -> &mut Self {
        self.url = url.into();
        self
    }

    /// Discord Application ID that the client will use when connecting to Discord.
    ///
    /// Defaults to `"1053747938519679018"`.
    pub fn client_id<T: Into<String>>(&mut self, client_id: T) -> &mut Self {
        self.client_id = client_id.into();
        self
    }

    /// Jellyfin API Key that will be used to gather data about what is being played.
    ///
    /// Has no default.
    pub fn api_key<T: Into<String>>(&mut self, api_key: T) -> &mut Self {
        self.api_key = api_key.into();
        self
    }

    /// Controls the use of certificate validation in reqwest.
    ///
    /// Defaults to `false`.
    pub fn self_signed(&mut self, self_signed: bool) -> &mut Self {
        self.self_signed = self_signed;
        self
    }

    /// Usernames that should be matched when checking Jellyfin sessions.
    ///
    /// Has no default.
    ///
    /// # Warning
    /// This overwrites the value set in `ClientBuilder::Username()`,
    /// only one of these 2 should be used
    pub fn usernames(&mut self, usernames: Vec<String>) -> &mut Self {
        self.usernames = usernames;
        self
    }

    /// same as `ClientBuilder::Usernames()` but will only accept a single username
    ///
    /// Has no default.
    ///
    /// # Warning
    /// This overwrites the value set in `ClientBuilder::Usernames()`,
    /// only one of these 2 should be used
    pub fn username<T: Into<String>>(&mut self, username: T) -> &mut Self {
        self.usernames = vec![username.into()];
        self
    }

    /// buttons to be displayed on the activity.
    /// Pass an empty `Vec::new()` to display no buttons
    ///
    /// Defaults to dynamic buttons generated from the Jellyfin session.
    pub fn buttons(&mut self, buttons: Vec<Button>) -> &mut Self {
        self.buttons = Some(buttons);
        self
    }

    /// Splits season and episode numbers with a dash.
    ///
    /// Defaults to `false`.
    ///
    /// # Example
    /// S1E1 Pilot -> S1 - E1 Pilot
    pub fn episode_divider(&mut self, val: bool) -> &mut Self {
        self.episode_divider = val;
        self
    }

    /// Adds leading 0's to season and episode numbers.
    ///
    /// Defaults to `false`.
    ///
    /// # Example
    /// S1E1 Pilot -> S01E01 Pilot
    pub fn episode_prefix(&mut self, val: bool) -> &mut Self {
        self.episode_prefix = val;
        self
    }

    /// Removes the episode name from the activity.
    ///
    /// Defaults to `false`.
    ///
    /// # Example
    /// S1E1 Pilot -> S1E1
    pub fn episode_simple(&mut self, val: bool) -> &mut Self {
        self.episode_simple = val;
        self
    }

    pub fn music_separator<T: Into<String>>(&mut self, separator: T) -> &mut Self {
        self.music_separator = separator.into();
        self
    }

    pub fn music_display(&mut self, display: DisplayFormat) -> &mut Self {
        self.music_display = display;
        self
    }

    pub fn music_status_display_type(&mut self, status_type: StatusType) -> &mut Self {
        self.music_status_display_type = status_type;
        self
    }

    pub fn movies_separator<T: Into<String>>(&mut self, separator: T) -> &mut Self {
        self.movies_separator = separator.into();
        self
    }

    pub fn movies_display(&mut self, display: DisplayFormat) -> &mut Self {
        self.movies_display = display;
        self
    }

    pub fn movies_status_display_type(&mut self, status_type: StatusType) -> &mut Self {
        self.movies_status_display_type = status_type;
        self
    }

    pub fn episodes_separator<T: Into<String>>(&mut self, separator: T) -> &mut Self {
        self.episodes_separator = separator.into();
        self
    }

    pub fn episodes_display(&mut self, display: DisplayFormat) -> &mut Self {
        self.episodes_display = display;
        self
    }

    pub fn episodes_status_display_type(&mut self, status_type: StatusType) -> &mut Self {
        self.episodes_status_display_type = status_type;
        self
    }

    /// Blacklist certain `MediaType`s so they don't display.
    ///
    /// Defaults to `Vec::new()`.
    pub fn blacklist_media_types(&mut self, media_types: Vec<MediaType>) -> &mut Self {
        self.blacklist_media_types = media_types;
        self
    }

    /// Blacklist certain libraries so they don't display.
    ///
    /// Defaults to `Vec::new()`.
    pub fn blacklist_libraries(&mut self, libraries: Vec<String>) -> &mut Self {
        self.blacklist_libraries = libraries;
        self
    }

    /// Show activity when paused.
    ///
    /// Defaults to `true`.
    pub fn show_paused(&mut self, val: bool) -> &mut Self {
        self.show_paused = val;
        self
    }

    /// Show images from jellyfin on the activity.
    ///
    /// Defaults to `false`.
    pub fn show_images(&mut self, val: bool) -> &mut Self {
        self.show_images = val;
        self
    }

    /// Use imgur for images, uploads images from jellyfin to imgur and stores the imgur links in a local cache
    ///
    /// Defaults to `false`.
    pub fn use_imgur(&mut self, val: bool) -> &mut Self {
        self.use_imgur = val;
        self
    }

    /// Imgur client id, used to upload images through their API.
    ///
    /// Empty by default.
    pub fn imgur_client_id<T: Into<String>>(&mut self, client_id: T) -> &mut Self {
        self.imgur_client_id = client_id.into();
        self
    }

    /// Where to store the URLs to images uploaded to imgur.
    /// Having this cache lets you avoid uploading the same image several times to their service.
    ///
    /// Empty by default.
    ///
    /// # Warning
    /// Setting this to something like `/dev/null` is **NOT** recommended,
    /// jellyfin-rpc will upload the image every time you call `Client::set_activity()`
    /// if it can't find the image its looking for in the cache.
    pub fn imgur_urls_file_location<T: Into<String>>(&mut self, location: T) -> &mut Self {
        self.imgur_urls_file_location = location.into();
        self
    }

    /// Use litterbox.catbox.moe for images, uploads images from jellyfin to litterbox and stores the litterbox links in a local cache
    ///
    /// Defaults to `false`.
    pub fn use_litterbox(&mut self, val: bool) -> &mut Self {
        self.use_litterbox = val;
        self
    }

    /// Where to store the URLs to images uploaded to litterbox.
    /// Having this cache lets you avoid uploading the same image several times to their service.
    ///
    /// Empty by default.
    pub fn litterbox_urls_file_location<T: Into<String>>(&mut  self, location: T) -> &mut Self {
        self.litterbox_urls_file_location = location.into();
        self
    }

    /// Process images before uploading to imgur or litterbox
    ///
    /// Defaults to `true`.
    pub fn process_images(&mut self, val: bool) -> &mut Self {
        self.process_images = val;
        self
    }

    /// Text to be displayed when hovering the large activity image in Discord
    ///
    /// Empty by default
    pub fn large_image_text<T: Into<String>>(&mut self, text: T) -> &mut Self {
        self.large_image_text = text.into();
        self
    }

    /// Builds a client from the options specified in the builder.
    ///
    /// # Example
    /// ```
    /// use jellyfin_rpc::ClientBuilder;
    ///
    /// let mut builder = ClientBuilder::new();
    /// builder.api_key("abcd1234")
    ///     .url("https://jellyfin.example.com")
    ///     .username("user");
    ///
    /// let mut client = builder.build().unwrap();
    /// ```
    pub fn build(self) -> JfResult<Client> {
        if self.url.is_empty() || self.usernames.is_empty() || self.api_key.is_empty() {
            return Err(Box::new(JfError::MissingRequiredValues));
        }

        let mut headers = HeaderMap::new();

        headers.insert(
            AUTHORIZATION,
            format!("MediaBrowser Token=\"{}\"", self.api_key).parse()?,
        );
        headers.insert("X-Emby-Token", self.api_key.parse()?);

        Ok(Client {
            discord_ipc_client: DiscordIpcClient::new(&self.client_id),
            url: self.url.parse()?,
            reqwest: reqwest::blocking::Client::builder()
                .default_headers(headers)
                .danger_accept_invalid_certs(self.self_signed)
                .build()?,
            usernames: self.usernames,
            buttons: self.buttons,
            session: None,
            music_display_options: DisplayOptions {
                separator: self.music_separator,
                display: self.music_display,
                status_display_type: self.music_status_display_type,
            },
            movies_display_options: DisplayOptions {
                separator: self.movies_separator,
                display: self.movies_display,
                status_display_type: self.movies_status_display_type,
            },
            episodes_display_options: DisplayOptions {
                separator: self.episodes_separator,
                display: self.episodes_display,
                status_display_type: self.episodes_status_display_type,
            },
            blacklist: Blacklist {
                media_types: self.blacklist_media_types,
                libraries_names: self.blacklist_libraries,
                libraries: BlacklistedLibraries::Uninitialized,
            },
            show_paused: self.show_paused,
            show_images: self.show_images,
            imgur_options: ImgurOptions {
                enabled: self.use_imgur,
                client_id: self.imgur_client_id,
                urls_location: self.imgur_urls_file_location,
            },
            litterbox_options: LitterboxOptions {
                enabled: self.use_litterbox,
                urls_location: self.litterbox_urls_file_location,
            },
            process_images: self.process_images,
            large_image_text: self.large_image_text,
        })
    }
}
