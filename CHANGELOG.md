# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.4] - 2023-08-21

### Added

- Implemented the missing track-related endpoints.
- Added an optional `tracks` method to the builder for creating playlists. It takes a slice of track or episode URIs and makes two additional API calls
to add tracks to the newly created playlist: one for adding the tracks and one for getting their details.

### Removed

- Removed the `fields` option from the get-playlist builder, as it would be used to filter responses, in which case a `Playlist` couldn't be deserialized
properly. Users can do filtering by accessing specific fields of the `Playlist` struct anyway.


## [0.3.3] - 2023-08-20

### Added

- Added endpoints: [playlists](https://developer.spotify.com/documentation/web-api/reference/get-playlist),
  [search](https://developer.spotify.com/documentation/web-api/reference/search),
  [shows](https://developer.spotify.com/documentation/web-api/reference/get-a-show),
  [tracks*](https://developer.spotify.com/documentation/web-api/reference/get-track),
  [users](https://developer.spotify.com/documentation/web-api/reference/get-current-users-profile).
  
   *a few endpoints are still not implemented
- `Client::from_refresh_token()` method that allows you get a new client using an existing refresh token.
- Internal `BoundedU32<const MIN: u32, const MAX: u32>` type that clamps a u32 to `MIN, MAX` upon creation. `Limit` is a `BoundedU32<1, 50>` -
  what Spotify uses for its limits. u32s passed by users are converted to said type. This might be unpredictable behaviour for the users,
  but it will be documented and I believe it's for the better - however, I am open to suggestions and might remove it in the future.

### Changed

- Methods with empty API responses now return [`Nil`](https://docs.rs/spotify-rs/latest/spotify_rs/struct.Nil.html) instead of `()`, in order to make deserialization from empty responses easy while keeping flexibility.

## [0.3.2] - 2023-08-17

### Changed

- Changed the approach to the builders, the public API now being endpoint-oriented.

  Getting an album with the optional `market` parameter set:
  ```rs
  // before
  let album = spotify.get_album(AlbumQuery::new("id").market("GB")).await?;

  // after
  let album = spotify.album("id").market("RO").get().await?;
  ```


## [0.3.1] - 2023-08-10

### Changed

- Changed the signature of methods that take several IDs to allow for more flexibility in the arguments.

## [0.3.0] - 2023-08-07

### Added

- Added two new authorisation flows: Authorisation Code Flow (no PKCE) and Client Credentials Flow.
- Added endpoints:
- All [artist endpoints](https://developer.spotify.com/documentation/web-api/reference/get-an-artist)
- All [audiobook endpoints](https://developer.spotify.com/documentation/web-api/reference/get-an-audiobook)
- All [category endpoints](https://developer.spotify.com/documentation/web-api/reference/get-categories)
- All [chapter endpoints](https://developer.spotify.com/documentation/web-api/reference/get-a-chapter)
      
    *Note: they return `500 Server error`, which is an issue with the API.*
- All [episode endpoints](https://developer.spotify.com/documentation/web-api/reference/get-an-episode)
- The [genre endpoint](https://developer.spotify.com/documentation/web-api/reference/get-recommendation-genres)

### Changed

- Removed duplicate token refresh methods from `Client<AuthCodeGrantPKCEFlow>` and the new `Client<AuthCodeGrantFlow>` since they were the same as the generic implementation.
- Client now takes a new generic type parameter: `A: AuthenticatedState`. The `Token` type now implements said trait and endpoint methods are *only* available on the `Client<Token, F>` type now. This ensures endpoint methods can only be called after authentication (not taking into account expired tokens).
- Removed the existing types for the Implicit Grant flow as it will not be implemented: it's very basic not recommended.
- Removed `oauth2::AuthorizationUrl` from the public API and re-exported `oauth2::RedirectUrl`.

## [0.2.0] - 2023-08-06

### Changed

- Moved from `Option<T>` for optional parameters to builders for each endpoint.

## [0.1.2] - 2023-08-04

### Added

- All [album endpoints](https://developer.spotify.com/documentation/web-api/reference/get-an-album) from the Spotify API.
- `Spotify` variant for the `Error` enum and deserializing into a struct that is converted into said variant.
- `RefreshUnavailable` variant of the `Error` enum, for when you're requesting a refresh but the auth flow doesn't support it.
- Auto refresh for the token (if enabled).
- Created CHANGELOG.md.

### Changed

- Slight internal changes to the model.
- The `flow` field of the client is gone, instead using `PhantomData` now.

## [0.1.1] - 2023-08-03

### Changed

- Added additional `Cargo.toml` metadata.

## [0.1.0] - 2023-08-03

- Initial release.