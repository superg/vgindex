pub mod about;
pub mod api;
pub mod auth_routes;
pub mod disc_edit;
pub mod disc_view;
pub mod discs;
pub mod downloads;
pub mod feeds;
pub mod main_page;
pub mod queue;

pub fn system_display_name(code: &str) -> String {
    match code {
        "AUDIO-CD"     => "Audio CD",
        "BD-VIDEO"     => "BD-Video",
        "CDI"          => "CD-i",
        "CHIHIRO"      => "Chihiro",
        "DVD-VIDEO"    => "DVD-Video",
        "ENHANCED-CD"  => "Enhanced CD",
        "GAMEWAVE"     => "Game Wave",
        "HDDVD-VIDEO"  => "HD DVD-Video",
        "IXL"          => "iXL",
        "LINDBERGH"    => "Lindbergh",
        "NAOMI"        => "Naomi",
        "NAOMI2"       => "Naomi 2",
        "PALM"         => "Palm OS",
        "PHOTO-CD"     => "Photo CD",
        "PIPPIN"       => "Pippin",
        "QUIZARD"      => "Quizard",
        "VFLASH"       => "V.Flash",
        "WII"          => "Wii",
        "WIIU"         => "Wii U",
        "XBOX"         => "Xbox",
        "XBOX360"      => "Xbox 360",
        "XBOXONE"      => "Xbox One",
        "XBOXSX"       => "Xbox SX",
        _              => code,
    }.to_string()
}

use axum::Router;
use crate::AppState;

pub fn build_router() -> Router<AppState> {
    Router::new()
        .merge(main_page::routes())
        .merge(auth_routes::routes())
        .merge(crate::auth::oidc::routes())
        .merge(discs::routes())
        .merge(disc_view::routes())
        .merge(disc_edit::routes())
        .merge(downloads::routes())
        .merge(queue::routes())
        .merge(feeds::routes())
        .merge(api::routes())
        .merge(about::routes())
}
