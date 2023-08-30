use std::marker::PhantomData;

use base64::{engine::general_purpose, Engine};
use oauth2::{
    basic::{
        BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
        BasicTokenType,
    },
    reqwest::async_http_client,
    AuthUrl, AuthorizationCode, CsrfToken, PkceCodeChallenge, RedirectUrl, RefreshToken,
    StandardRevocableToken,
};
use reqwest::{header::CONTENT_LENGTH, Method};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;

use crate::{
    auth::{
        AuthCodeGrantFlow, AuthCodeGrantPKCEFlow, AuthFlow, AuthenticationState, Authorisation,
        AuthorisationPKCE, Authorised, ClientCredsGrantFlow, Scope, Token, UnAuthenticated,
    },
    body_list,
    endpoint::{
        album::{
            AlbumEndpoint, AlbumTracksEndpoint, AlbumsEndpoint, NewReleasesEndpoint,
            SavedAlbumsEndpoint,
        },
        artist::ArtistEndpoint,
        audiobook::{
            AudiobookChaptersEndpoint, AudiobookEndpoint, AudiobooksEndpoint, ChapterEndpoint,
            ChaptersEndpoint, SavedAudiobooksEndpoint,
        },
        category::{BrowseCategoriesEndpoint, BrowseCategoryEndpoint},
        player::{
            AddItemToQueueEndpoint, RecentlyPlayedTracksEndpoint, RepeatMode,
            SeekToPositionEndpoint, SetPlaybackVolumeEndpoint, SetRepeatModeEndpoint,
            StartPlaybackEndpoint, ToggleShuffleEndpoint, TransferPlaybackEndpoint,
        },
        playlist::{
            AddPlaylistItemsEndpoint, CategoryPlaylistsEndpoint, ChangePlaylistDetailsEndpoint,
            CreatePlaylistEndpoint, CurrentUserPlaylistsEndpoint, FeaturedPlaylistsEndpoint,
            PlaylistEndpoint, PlaylistItemsEndpoint, RemovePlaylistItemsEndpoint,
            UpdatePlaylistItemsEndpoint, UserPlaylistsEndpoint,
        },
        search::SearchEndpoint,
        show::{
            EpisodeEndpoint, EpisodesEndpoint, SavedEpisodesEndpoint, SavedShowsEndpoint,
            ShowEndpoint, ShowEpisodesEndpoint, ShowsEndpoint,
        },
        track::{
            Feature, RecommendationsEndpoint, SavedTracksEndpoint, Seed, SeedArtists, SeedType,
            TrackEndpoint, TracksEndpoint,
        },
        user::{
            FollowPlaylistBuilder, FollowUserOrArtistEndpoint, FollowedArtistsBuilder,
            UserTopItemsEndpoint,
        },
        Builder, Endpoint,
    },
    error::{Error, SpotifyError},
    model::{
        artist::{Artist, Artists},
        audio::{AudioAnalysis, AudioFeatures, AudioFeaturesResult},
        market::Markets,
        player::{Device, Devices, PlaybackState, Queue},
        recommendation::Genres,
        search::Item,
        user::{User, UserItemType},
        Image,
    },
    query_list, Nil, Result,
};

pub(crate) type OAuthClient = oauth2::Client<
    BasicErrorResponse,
    Token,
    BasicTokenType,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
>;

#[doc(hidden)]
pub(crate) enum Body<P: Serialize = ()> {
    Json(P),
    File(Vec<u8>),
}

#[derive(Debug)]
pub struct Client<A: AuthenticationState, F: AuthFlow> {
    pub auto_refresh: bool,
    pub(crate) auth: A,
    pub(crate) oauth: OAuthClient,
    pub(crate) http: reqwest::Client,
    marker: PhantomData<F>,
}

impl<F: AuthFlow> Client<UnAuthenticated, F> {
    pub fn new(
        auth_flow: F,
        redirect_uri: RedirectUrl,
        auto_refresh: bool,
    ) -> Client<UnAuthenticated, F> {
        let oauth_client = OAuthClient::new(
            auth_flow.client_id(),
            auth_flow.client_secret(),
            AuthUrl::new("https://accounts.spotify.com/authorize".to_owned()).unwrap(),
            auth_flow.token_url(),
        )
        .set_redirect_uri(redirect_uri);

        Client {
            auto_refresh,
            auth: UnAuthenticated,
            oauth: oauth_client,
            http: reqwest::Client::new(),
            marker: PhantomData,
        }
    }
}

impl<F: AuthFlow> Client<Token, F> {
    pub async fn from_refresh_token<I>(
        auth_flow: F,
        redirect_uri: RedirectUrl,
        auto_refresh: bool,
        scopes: I,
        refresh_token: String,
    ) -> Result<Client<Token, F>>
    where
        I: IntoIterator,
        I::Item: Into<Scope>,
    {
        let oauth_client = OAuthClient::new(
            auth_flow.client_id(),
            auth_flow.client_secret(),
            AuthUrl::new("https://accounts.spotify.com/authorize".to_owned()).unwrap(),
            auth_flow.token_url(),
        )
        .set_redirect_uri(redirect_uri);

        let refresh_token = RefreshToken::new(refresh_token);

        let token = oauth_client
            .exchange_refresh_token(&refresh_token)
            .add_scopes(scopes.into_iter().map(|i| i.into().0))
            .request_async(async_http_client)
            .await?
            .set_timestamps();

        Ok(Client {
            auto_refresh,
            auth: token,
            oauth: oauth_client,
            http: reqwest::Client::new(),
            marker: PhantomData,
        })
    }

    pub fn access_token(&self) -> &str {
        self.auth.access_token.secret()
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.auth
            .refresh_token
            .as_ref()
            .map(|t| t.secret().as_str())
    }

    pub async fn request_refresh_token(&mut self) -> Result<()> {
        let Some(refresh_token) = &self.auth.refresh_token else {
            return Err(Error::RefreshUnavailable);
        };

        let token = self
            .oauth
            .exchange_refresh_token(refresh_token)
            .request_async(async_http_client)
            .await?
            .set_timestamps();

        self.auth = token;
        Ok(())
    }

    pub(crate) async fn request<P: Serialize, T: DeserializeOwned>(
        &mut self,
        method: Method,
        endpoint: String,
        query: Option<P>,
        body: Option<Body<P>>,
    ) -> Result<T> {
        if self.auth.is_expired() {
            if self.auto_refresh {
                self.request_refresh_token().await?;
            } else {
                return Err(Error::ExpiredToken);
            }
        }

        let mut req = self
            .http
            .request(method, format!("https://api.spotify.com/v1{endpoint}"))
            .bearer_auth(self.auth.access_token.secret());

        if let Some(q) = query {
            req = req.query(&q);
        }

        if let Some(b) = body {
            match b {
                Body::Json(j) => req = req.json(&j),
                Body::File(f) => req = req.body(f),
            }
        } else {
            // Used because Spotify wants a Content-Length header for the PUT /audiobooks/me endpoint even though there is no body
            // If not supplied, it will return an error in the form of HTML (not JSON), which I believe to be an issue on their end.
            // No other endpoints so far behave this way.
            req = req.header(CONTENT_LENGTH, 0);
        }

        let res = req.send().await?;

        if res.status().is_success() {
            Ok(res.json().await?)
        } else {
            Err(res.json::<SpotifyError>().await?.into())
        }
    }

    pub(crate) async fn get<P: Serialize, T: DeserializeOwned>(
        &mut self,
        endpoint: String,
        query: impl Into<Option<P>>,
    ) -> Result<T> {
        self.request(Method::GET, endpoint, query.into(), None)
            .await
    }

    pub(crate) async fn post<P: Serialize, T: DeserializeOwned>(
        &mut self,
        endpoint: String,
        body: impl Into<Option<Body<P>>>,
    ) -> Result<T> {
        self.request(Method::POST, endpoint, None, body.into())
            .await
    }

    pub(crate) async fn put<P: Serialize, T: DeserializeOwned>(
        &mut self,
        endpoint: String,
        body: impl Into<Option<Body<P>>>,
    ) -> Result<T> {
        self.request(Method::PUT, endpoint, None, body.into()).await
    }

    pub(crate) async fn delete<P: Serialize, T: DeserializeOwned>(
        &mut self,
        endpoint: String,
        body: impl Into<Option<Body<P>>>,
    ) -> Result<T> {
        self.request(Method::DELETE, endpoint, None, body.into())
            .await
    }

    fn builder<E: Endpoint>(&mut self, endpoint: E) -> Builder<'_, F, E> {
        Builder {
            spotify: self,
            endpoint,
        }
    }

    pub fn album(&mut self, id: &str) -> Builder<'_, F, AlbumEndpoint> {
        self.builder(AlbumEndpoint {
            id: id.to_owned(),
            market: None,
        })
    }

    pub fn albums<T: AsRef<str>>(&mut self, ids: &[T]) -> Builder<'_, F, AlbumsEndpoint> {
        self.builder(AlbumsEndpoint {
            ids: query_list(ids),
            market: None,
        })
    }

    pub fn album_tracks(&mut self, album_id: &str) -> Builder<'_, F, AlbumTracksEndpoint> {
        self.builder(AlbumTracksEndpoint {
            id: album_id.to_owned(),
            ..Default::default()
        })
    }

    pub fn new_releases(&mut self) -> Builder<'_, F, NewReleasesEndpoint> {
        self.builder(NewReleasesEndpoint::default())
    }

    pub fn artist(&mut self, id: &str) -> Builder<'_, F, ArtistEndpoint> {
        self.builder(ArtistEndpoint { id: id.to_owned() })
    }

    pub async fn get_artists<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Vec<Artist>> {
        self.get("/artists".to_owned(), [("ids", query_list(ids))])
            .await
            .map(|a: Artists| a.artists)
    }

    pub fn audiobook(&mut self, id: &str) -> Builder<'_, F, AudiobookEndpoint> {
        self.builder(AudiobookEndpoint {
            id: id.to_owned(),
            market: None,
        })
    }

    pub fn audiobooks<T: AsRef<str>>(&mut self, ids: &[T]) -> Builder<'_, F, AudiobooksEndpoint> {
        self.builder(AudiobooksEndpoint {
            ids: query_list(ids),
            market: None,
        })
    }

    pub fn audiobook_chapters(
        &mut self,
        audiobook_id: &str,
    ) -> Builder<'_, F, AudiobookChaptersEndpoint> {
        self.builder(AudiobookChaptersEndpoint {
            id: audiobook_id.to_owned(),
            ..Default::default()
        })
    }

    pub fn browse_category(&mut self, id: &str) -> Builder<'_, F, BrowseCategoryEndpoint> {
        self.builder(BrowseCategoryEndpoint {
            id: id.to_owned(),
            ..Default::default()
        })
    }

    pub fn browse_categories(&mut self) -> Builder<'_, F, BrowseCategoriesEndpoint> {
        self.builder(BrowseCategoriesEndpoint::default())
    }

    /// *Note: Spotify's API returns `500 Server error`.*
    pub fn chapter(&mut self, id: &str) -> Builder<'_, F, ChapterEndpoint> {
        self.builder(ChapterEndpoint {
            id: id.to_owned(),
            market: None,
        })
    }

    /// *Note: Spotify's API returns `500 Server error`.*
    pub fn chapters<T: AsRef<str>>(&mut self, ids: &[T]) -> Builder<'_, F, ChaptersEndpoint> {
        self.builder(ChaptersEndpoint {
            ids: query_list(ids),
            market: None,
        })
    }

    pub fn episode(&mut self, id: &str) -> Builder<'_, F, EpisodeEndpoint> {
        self.builder(EpisodeEndpoint {
            id: id.to_owned(),
            market: None,
        })
    }

    pub fn episodes<T: AsRef<str>>(&mut self, ids: &[T]) -> Builder<'_, F, EpisodesEndpoint> {
        self.builder(EpisodesEndpoint {
            ids: query_list(ids),
            market: None,
        })
    }

    pub async fn get_genre_seeds(&mut self) -> Result<Vec<String>> {
        self.get::<(), _>("/recommendations/available-genre-seeds".to_owned(), None)
            .await
            .map(|g: Genres| g.genres)
    }

    pub async fn get_available_markets(&mut self) -> Result<Vec<String>> {
        self.get::<(), _>("/markets".to_owned(), None)
            .await
            .map(|m: Markets| m.markets)
    }

    pub fn playlist(&mut self, id: &str) -> Builder<'_, F, PlaylistEndpoint> {
        self.builder(PlaylistEndpoint {
            id: id.to_owned(),
            ..Default::default()
        })
    }

    pub fn change_playlist_details(
        &mut self,
        id: &str,
    ) -> Builder<'_, F, ChangePlaylistDetailsEndpoint> {
        self.builder(ChangePlaylistDetailsEndpoint {
            id: id.to_owned(),
            ..Default::default()
        })
    }

    pub fn playlist_items(&mut self, id: &str) -> Builder<'_, F, PlaylistItemsEndpoint> {
        self.builder(PlaylistItemsEndpoint {
            id: id.to_owned(),
            ..Default::default()
        })
    }

    pub fn update_playlist_items(
        &mut self,
        id: &str,
        range_start: u32,
        insert_before: u32,
    ) -> Builder<'_, F, UpdatePlaylistItemsEndpoint> {
        self.builder(UpdatePlaylistItemsEndpoint {
            id: id.to_owned(),
            range_start,
            insert_before,
            ..Default::default()
        })
    }

    pub fn add_items_to_playlist<T: ToString>(
        &mut self,
        id: &str,
        item_uris: &[T],
    ) -> Builder<'_, F, AddPlaylistItemsEndpoint> {
        self.builder(AddPlaylistItemsEndpoint {
            id: id.to_owned(),
            uris: item_uris.iter().map(ToString::to_string).collect(),
            position: None,
        })
    }

    pub fn remove_playlist_items<T: AsRef<str>>(
        &mut self,
        id: &str,
        item_uris: &[T],
    ) -> Builder<'_, F, RemovePlaylistItemsEndpoint> {
        let tracks = item_uris
            .iter()
            .map(|u| json!({ "uri": u.as_ref() }))
            .collect();

        self.builder(RemovePlaylistItemsEndpoint {
            id: id.to_owned(),
            tracks,
            snapshot_id: None,
        })
    }

    pub fn user_playlists(&mut self, user_id: &str) -> Builder<'_, F, UserPlaylistsEndpoint> {
        self.builder(UserPlaylistsEndpoint {
            id: user_id.to_owned(),
            ..Default::default()
        })
    }

    pub fn create_playlist(
        &mut self,
        user_id: &str,
        name: &str,
    ) -> Builder<'_, F, CreatePlaylistEndpoint> {
        self.builder(CreatePlaylistEndpoint {
            user_id: user_id.to_owned(),
            name: name.to_owned(),
            ..Default::default()
        })
    }

    pub fn featured_playlists(&mut self) -> Builder<'_, F, FeaturedPlaylistsEndpoint> {
        self.builder(FeaturedPlaylistsEndpoint::default())
    }

    pub fn category_playlists(
        &mut self,
        category_id: &str,
    ) -> Builder<'_, F, CategoryPlaylistsEndpoint> {
        self.builder(CategoryPlaylistsEndpoint {
            id: category_id.to_owned(),
            ..Default::default()
        })
    }

    pub async fn get_playlist_image(&mut self, id: &str) -> Result<Vec<Image>> {
        self.get::<(), _>(format!("/playlists/{id}/images"), None)
            .await
    }

    pub async fn add_playlist_image(&mut self, id: &str, image: &[u8]) -> Result<Nil> {
        let encoded_image = general_purpose::STANDARD.encode(image).into_bytes();
        let body = <Body>::File(encoded_image);

        self.put(format!("/playlists/{id}/images"), body).await
    }

    pub fn search(&mut self, query: &str, item_types: &[Item]) -> Builder<'_, F, SearchEndpoint> {
        let r#type = query_list(item_types);

        self.builder(SearchEndpoint {
            query: query.to_owned(),
            r#type,
            ..Default::default()
        })
    }

    pub fn show(&mut self, id: &str) -> Builder<'_, F, ShowEndpoint> {
        self.builder(ShowEndpoint {
            id: id.to_owned(),
            market: None,
        })
    }

    pub fn shows<T: AsRef<str>>(&mut self, ids: &[T]) -> Builder<'_, F, ShowsEndpoint> {
        self.builder(ShowsEndpoint {
            ids: query_list(ids),
            market: None,
        })
    }

    pub fn show_episodes(&mut self, show_id: &str) -> Builder<'_, F, ShowEpisodesEndpoint> {
        self.builder(ShowEpisodesEndpoint {
            show_id: show_id.to_owned(),
            ..Default::default()
        })
    }

    pub fn track(&mut self, id: &str) -> Builder<'_, F, TrackEndpoint> {
        self.builder(TrackEndpoint {
            id: id.to_owned(),
            market: None,
        })
    }

    pub fn tracks<T: AsRef<str>>(&mut self, ids: &[T]) -> Builder<'_, F, TracksEndpoint> {
        self.builder(TracksEndpoint {
            ids: query_list(ids),
            market: None,
        })
    }

    pub async fn get_track_audio_features(&mut self, id: &str) -> Result<AudioFeatures> {
        self.get::<(), _>(format!("/audio-features/{id}"), None)
            .await
    }

    pub async fn get_tracks_audio_features<T: AsRef<str>>(
        &mut self,
        ids: &[T],
    ) -> Result<Vec<AudioFeatures>> {
        self.get("/audio-features".to_owned(), [("ids", query_list(ids))])
            .await
            .map(|a: AudioFeaturesResult| a.audio_features)
    }

    pub async fn get_track_audio_analysis(&mut self, id: &str) -> Result<AudioAnalysis> {
        self.get::<(), _>(format!("/audio-analysis/{id}"), None)
            .await
    }

    pub fn recommendations<S: SeedType, T: AsRef<str>>(
        &mut self,
        seed: Seed<T, S>,
    ) -> Builder<'_, F, RecommendationsEndpoint<S>> {
        let (seed_artists, seed_genres, seed_tracks) = match seed {
            Seed::Artists(ids, _) => (Some(query_list(ids)), None, None),
            Seed::Genres(genres, _) => (None, Some(query_list(genres)), None),
            Seed::Tracks(ids, _) => (None, None, Some(query_list(ids))),
        };

        self.builder(RecommendationsEndpoint {
            seed_artists,
            seed_genres,
            seed_tracks,
            limit: None,
            market: None,
            features: None,
            marker: PhantomData,
        })
    }

    pub async fn get_user(&mut self, id: &str) -> Result<User> {
        self.get::<(), _>(format!("/users/{id}"), None).await
    }

    pub async fn check_if_users_follow_playlist<T: AsRef<str>>(
        &mut self,
        playlist_id: &str,
        user_ids: &[T],
    ) -> Result<Vec<bool>> {
        self.get(
            format!("/playlists/{playlist_id}/followers/contains"),
            [("ids", query_list(user_ids))],
        )
        .await
    }
}

impl<F: AuthFlow + Authorised> Client<Token, F> {
    pub fn saved_albums(&mut self) -> Builder<'_, F, SavedAlbumsEndpoint> {
        self.builder(SavedAlbumsEndpoint::default())
    }

    pub async fn save_albums<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.put("/me/albums".to_owned(), body_list("ids", ids))
            .await
    }

    pub async fn remove_saved_albums<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.delete("/me/albums".to_owned(), body_list("ids", ids))
            .await
    }

    pub async fn check_saved_albums<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Vec<bool>> {
        self.get("/me/albums/contains".to_owned(), [("ids", query_list(ids))])
            .await
    }

    pub fn saved_audiobooks(&mut self) -> Builder<'_, F, SavedAudiobooksEndpoint> {
        self.builder(SavedAudiobooksEndpoint::default())
    }

    pub async fn save_audiobooks<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.put::<(), _>(format!("/me/audiobooks?ids={}", query_list(ids)), None)
            .await
    }

    pub async fn remove_saved_audiobooks<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.delete::<(), _>(format!("/me/audiobooks?ids={}", query_list(ids)), None)
            .await
    }

    pub async fn check_saved_audiobooks<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Vec<bool>> {
        self.get(
            "/me/audiobooks/contains".to_owned(),
            [("ids", query_list(ids))],
        )
        .await
    }

    pub fn saved_episodes(&mut self) -> Builder<'_, F, SavedEpisodesEndpoint> {
        self.builder(SavedEpisodesEndpoint::default())
    }

    pub async fn save_episodes<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.put("/me/episodes".to_owned(), body_list("ids", ids))
            .await
    }

    pub async fn remove_saved_episodes<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.delete("/me/episodes".to_owned(), body_list("ids", ids))
            .await
    }

    pub async fn check_saved_episodes<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Vec<bool>> {
        self.get::<(), _>(
            format!("/me/episodes/contains?ids={}", query_list(ids)),
            None,
        )
        .await
    }

    pub fn current_user_playlists(&mut self) -> Builder<'_, F, CurrentUserPlaylistsEndpoint> {
        self.builder(CurrentUserPlaylistsEndpoint::default())
    }

    pub fn saved_shows(&mut self) -> Builder<'_, F, SavedShowsEndpoint> {
        self.builder(SavedShowsEndpoint::default())
    }

    pub async fn save_shows<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.put("/me/shows".to_owned(), body_list("ids", ids))
            .await
    }

    pub async fn remove_saved_shows<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.delete("/me/shows".to_owned(), body_list("ids", ids))
            .await
    }

    pub async fn check_saved_shows<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Vec<bool>> {
        self.get("/me/shows/contains".to_owned(), [("ids", query_list(ids))])
            .await
    }

    pub fn saved_tracks(&mut self) -> Builder<'_, F, SavedTracksEndpoint> {
        self.builder(SavedTracksEndpoint::default())
    }

    pub async fn save_tracks<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.put("/me/tracks".to_owned(), body_list("ids", ids))
            .await
    }

    pub async fn remove_saved_tracks<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Nil> {
        self.delete("/me/tracks".to_owned(), body_list("ids", ids))
            .await
    }

    pub async fn check_saved_tracks<T: AsRef<str>>(&mut self, ids: &[T]) -> Result<Vec<bool>> {
        self.get("/me/tracks/contains".to_owned(), [("ids", query_list(ids))])
            .await
    }

    pub async fn get_current_user_profile(&mut self) -> Result<User> {
        self.get::<(), _>("/me".to_owned(), None).await
    }

    pub fn current_user_top_items(
        &mut self,
        r#type: UserItemType,
    ) -> Builder<'_, F, UserTopItemsEndpoint> {
        self.builder(UserTopItemsEndpoint {
            r#type,
            ..Default::default()
        })
    }

    pub fn follow_playlist(&mut self, id: &str) -> Builder<'_, F, FollowPlaylistBuilder> {
        self.builder(FollowPlaylistBuilder {
            id: id.to_owned(),
            public: None,
        })
    }

    pub async fn unfollow_playlist(&mut self, id: &str) -> Result<Nil> {
        self.delete::<(), _>(format!("/playlists/{id}/followers"), None)
            .await
    }

    pub fn followed_artists(&mut self) -> Builder<'_, F, FollowedArtistsBuilder> {
        // Currently only the "artist" type is supported, so it's hardcoded.
        self.builder(FollowedArtistsBuilder {
            r#type: "artist".to_owned(),
            ..Default::default()
        })
    }

    pub fn follow_artists<T: AsRef<str>>(
        &mut self,
        ids: &[T],
    ) -> Builder<'_, F, FollowUserOrArtistEndpoint> {
        self.builder(FollowUserOrArtistEndpoint {
            r#type: "artist".to_owned(),
            ids: ids.iter().map(|i| i.as_ref().to_owned()).collect(),
        })
    }

    pub fn follow_users<T: AsRef<str>>(
        &mut self,
        ids: &[T],
    ) -> Builder<'_, F, FollowUserOrArtistEndpoint> {
        self.builder(FollowUserOrArtistEndpoint {
            r#type: "user".to_owned(),
            ids: ids.iter().map(|i| i.as_ref().to_owned()).collect(),
        })
    }

    pub async fn get_playback_state(&mut self, market: Option<&str>) -> Result<PlaybackState> {
        let market = market.map(|m| [("market", m)]);
        self.get::<[(&str, &str); 1], _>("/me/player".to_owned(), market)
            .await
    }

    pub fn transfer_playback(
        &mut self,
        device_id: &str,
    ) -> Builder<'_, F, TransferPlaybackEndpoint> {
        self.builder(TransferPlaybackEndpoint {
            device_ids: vec![device_id.to_owned()],
            play: None,
        })
    }

    pub async fn get_available_devices(&mut self) -> Result<Vec<Device>> {
        self.get::<(), _>("/me/player/devices".to_owned(), None)
            .await
            .map(|d: Devices| d.devices)
    }

    pub async fn get_currently_playing_track(
        &mut self,
        market: Option<&str>,
    ) -> Result<PlaybackState> {
        let market = market.map(|m| [("market", m)]);
        self.get::<Option<[(&str, &str); 1]>, _>("/me/player/currently-playing".to_owned(), market)
            .await
    }

    pub fn start_playback(&mut self) -> Builder<'_, F, StartPlaybackEndpoint> {
        self.builder(StartPlaybackEndpoint::default())
    }

    pub async fn pause_playback(&mut self, device_id: Option<&str>) -> Result<Nil> {
        let device_id = device_id.map(|d| [("device_id", d)]);
        self.request(Method::PUT, "/me/player/pause".to_owned(), device_id, None)
            .await
    }

    pub async fn skip_to_next(&mut self, device_id: Option<&str>) -> Result<Nil> {
        let device_id = device_id.map(|d| [("device_id", d)]);
        self.request(Method::POST, "/me/player/next".to_owned(), device_id, None)
            .await
    }

    pub async fn skip_to_previous(&mut self, device_id: Option<&str>) -> Result<Nil> {
        let device_id = device_id.map(|d| [("device_id", d)]);
        self.request(
            Method::POST,
            "/me/player/previous".to_owned(),
            device_id,
            None,
        )
        .await
    }

    pub fn seek_to_position(&mut self, position: u32) -> Builder<'_, F, SeekToPositionEndpoint> {
        self.builder(SeekToPositionEndpoint {
            position_ms: position,
            device_id: None,
        })
    }

    /// *Note: This endpoint seems to be broken, returning 403 Forbidden "Player command failed: Restriction violated"*
    pub fn set_repeat_mode(
        &mut self,
        repeat_mode: RepeatMode,
    ) -> Builder<'_, F, SetRepeatModeEndpoint> {
        self.builder(SetRepeatModeEndpoint {
            state: repeat_mode,
            device_id: None,
        })
    }

    pub fn set_playback_volume(
        &mut self,
        volume: u32,
    ) -> Builder<'_, F, SetPlaybackVolumeEndpoint> {
        self.builder(SetPlaybackVolumeEndpoint {
            volume_percent: volume,
            device_id: None,
        })
    }

    /// *Note: This endpoint seems to be broken, returning 403 Forbidden "Player command failed: Restriction violated"*
    pub fn toggle_playback_shuffle(
        &mut self,
        shuffle: bool,
    ) -> Builder<'_, F, ToggleShuffleEndpoint> {
        self.builder(ToggleShuffleEndpoint {
            state: shuffle,
            device_id: None,
        })
    }

    pub fn recently_played_tracks(&mut self) -> Builder<'_, F, RecentlyPlayedTracksEndpoint> {
        self.builder(RecentlyPlayedTracksEndpoint::default())
    }

    pub async fn get_user_queue(&mut self) -> Result<Queue> {
        self.get::<(), _>("/me/player/queue".to_owned(), None).await
    }

    pub fn add_item_to_queue(&mut self, uri: &str) -> Builder<'_, F, AddItemToQueueEndpoint> {
        self.builder(AddItemToQueueEndpoint {
            uri: uri.to_owned(),
            device_id: None,
        })
    }
}

impl Client<UnAuthenticated, AuthCodeGrantPKCEFlow> {
    pub fn get_authorisation<I>(&self, scopes: I) -> AuthorisationPKCE
    where
        I: IntoIterator,
        I::Item: Into<Scope>,
    {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        let (auth_url, csrf_token) = self
            .oauth
            .authorize_url(CsrfToken::new_random)
            .add_scopes(scopes.into_iter().map(|i| i.into().0))
            .set_pkce_challenge(pkce_challenge)
            .url();

        AuthorisationPKCE {
            url: auth_url,
            csrf_token,
            pkce_verifier,
        }
    }

    pub async fn authenticate(
        self,
        auth: AuthorisationPKCE,
        auth_code: &str,
        csrf_state: &str,
    ) -> Result<Client<Token, AuthCodeGrantPKCEFlow>> {
        if csrf_state != auth.csrf_token.secret() {
            return Err(Error::InvalidStateParameter);
        }

        let token = self
            .oauth
            .exchange_code(AuthorizationCode::new(auth_code.to_owned()))
            .set_pkce_verifier(auth.pkce_verifier)
            .request_async(async_http_client)
            .await?
            .set_timestamps();

        Ok(Client {
            auto_refresh: self.auto_refresh,
            auth: token,
            oauth: self.oauth,
            http: self.http,
            marker: PhantomData,
        })
    }
}

impl Client<UnAuthenticated, AuthCodeGrantFlow> {
    pub fn get_authorisation<I>(&self, scopes: I) -> Authorisation
    where
        I: IntoIterator,
        I::Item: Into<Scope>,
    {
        let (auth_url, csrf_token) = self
            .oauth
            .authorize_url(CsrfToken::new_random)
            .add_scopes(scopes.into_iter().map(|i| i.into().0))
            .url();

        Authorisation {
            url: auth_url,
            csrf_token,
        }
    }

    pub async fn authenticate(
        self,
        auth: Authorisation,
        auth_code: &str,
        csrf_state: &str,
    ) -> Result<Client<Token, AuthCodeGrantFlow>> {
        if csrf_state != auth.csrf_token.secret() {
            return Err(Error::InvalidStateParameter);
        }

        let token = self
            .oauth
            .exchange_code(AuthorizationCode::new(auth_code.to_owned()))
            .request_async(async_http_client)
            .await?
            .set_timestamps();

        Ok(Client {
            auto_refresh: self.auto_refresh,
            auth: token,
            oauth: self.oauth,
            http: self.http,
            marker: PhantomData,
        })
    }
}

impl Client<UnAuthenticated, ClientCredsGrantFlow> {
    pub async fn authenticate<I>(self, scopes: I) -> Result<Client<Token, ClientCredsGrantFlow>>
    where
        I: IntoIterator,
        I::Item: Into<Scope>,
    {
        let token = self
            .oauth
            .exchange_client_credentials()
            .add_scopes(scopes.into_iter().map(|i| i.into().0))
            .request_async(async_http_client)
            .await?;

        Ok(Client {
            auto_refresh: self.auto_refresh,
            auth: token,
            oauth: self.oauth,
            http: self.http,
            marker: PhantomData,
        })
    }
}
