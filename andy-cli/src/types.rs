use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct CreateScreenRequest {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub dpi: i32,
    pub timeout_secs: u64,
    pub package: String,
}

#[derive(Serialize, Deserialize)]
pub struct ScreenInfo {
    pub name: String,
    pub display_id: i32,
    pub width: i32,
    pub height: i32,
    pub dpi: i32,
    pub assigned_package: String,
}

#[derive(Serialize)]
pub struct TapRequest {
    pub x: f32,
    pub y: f32,
}

#[derive(Serialize)]
pub struct SwipeRequest {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub duration_ms: i64,
}

#[derive(Serialize)]
pub struct TypeRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct KeyRequest {
    pub keycode: i32,
}


#[derive(Serialize)]
pub struct OpenUrlRequest {
    pub url: String,
}

#[derive(Serialize)]
pub struct WaitForIdleRequest {
    pub idle_timeout_ms: i64,
    pub global_timeout_ms: i64,
}
