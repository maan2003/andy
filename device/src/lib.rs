use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JObjectArray, JString, JValue};
use jni::{JNIEnv, JavaVM};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::time::{self, Instant};

const PORT: u16 = 21632;

struct VirtualScreen {
    display_id: i32,
    instance: GlobalRef,
    last_jpeg: Option<Vec<u8>>,
    width: i32,
    height: i32,
    dpi: i32,
    last_heartbeat: Instant,
    timeout_secs: u64,
    last_interaction: Option<Instant>,
    assigned_package: String,
}

struct ServerState {
    jvm: JavaVM,
    screen_class: GlobalRef,
    screens: HashMap<String, VirtualScreen>,
    a11y_bridge: GlobalRef,
}

type AppState = Arc<tokio::sync::Mutex<ServerState>>;

#[derive(Debug)]
struct AppError {
    message: String,
    status: StatusCode,
}

impl AppError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: StatusCode::NOT_FOUND,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

#[derive(Serialize, Deserialize)]
struct ScreenInfo {
    name: String,
    display_id: i32,
    width: i32,
    height: i32,
    dpi: i32,
    assigned_package: String,
}

#[derive(Deserialize)]
struct CreateScreenRequest {
    name: String,
    width: i32,
    height: i32,
    dpi: i32,
    timeout_secs: u64,
    package: String,
}

#[derive(Deserialize)]
struct TapRequest {
    x: f32,
    y: f32,
}

#[derive(Deserialize)]
struct SwipeRequest {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    duration_ms: i64,
}

#[derive(Deserialize)]
struct TypeRequest {
    text: String,
}

#[derive(Deserialize)]
struct KeyRequest {
    keycode: i32,
}

#[derive(Deserialize)]
struct OpenUrlRequest {
    url: String,
}


#[derive(Deserialize)]
struct WaitForIdleRequest {
    idle_timeout_ms: i64,
    global_timeout_ms: i64,
}

#[derive(Deserialize)]
struct NoWaitQuery {
    #[serde(default)]
    no_wait: bool,
}

fn encode_jpeg(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, AppError> {
    let mut buf = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut buf, 85);
    encoder
        .encode(
            rgba,
            width as u16,
            height as u16,
            jpeg_encoder::ColorType::Rgba,
        )
        .map_err(|e| AppError::new(format!("jpeg encode failed: {e}")))?;
    Ok(buf)
}

impl ServerState {
    fn get_screen(&self, name: &str) -> Result<&VirtualScreen, AppError> {
        self.screens
            .get(name)
            .ok_or_else(|| AppError::not_found(format!("screen {name} not found")))
    }

    fn get_screen_mut(&mut self, name: &str) -> Result<&mut VirtualScreen, AppError> {
        let screen = self
            .screens
            .get_mut(name)
            .ok_or_else(|| AppError::not_found(format!("screen {name} not found")))?;
        screen.last_heartbeat = Instant::now();
        Ok(screen)
    }

    fn with_env<T>(
        &self,
        f: impl FnOnce(&mut JNIEnv) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut env = self
            .jvm
            .attach_current_thread()
            .map_err(|e| AppError::new(format!("attach_current_thread failed: {e}")))?;
        f(&mut env)
    }

    fn create_screen(&mut self, req: &CreateScreenRequest) -> Result<ScreenInfo, AppError> {
        // Get-or-create: if screen with this name exists, reset heartbeat and return it
        if let Some(screen) = self.screens.get_mut(&req.name) {
            screen.last_heartbeat = Instant::now();
            return Ok(ScreenInfo {
                name: req.name.clone(),
                display_id: screen.display_id,
                width: screen.width,
                height: screen.height,
                dpi: screen.dpi,
                assigned_package: screen.assigned_package.clone(),
            });
        }

        let instance = self.with_env(|env| {
            let class: &JClass = self.screen_class.as_obj().into();
            let obj = env
                .new_object(
                    class,
                    "(III)V",
                    &[
                        JValue::Int(req.width),
                        JValue::Int(req.height),
                        JValue::Int(req.dpi),
                    ],
                )
                .map_err(|e| {
                    if let Some(exc_msg) = get_exception_message(env) {
                        AppError::new(format!("VirtualScreen constructor failed: {exc_msg}"))
                    } else {
                        AppError::new(format!("VirtualScreen constructor failed: {e}"))
                    }
                })?;
            let display_id = env
                .call_method(&obj, "getDisplayId", "()I", &[])
                .map_err(|e| AppError::new(format!("getDisplayId failed: {e}")))?
                .i()
                .map_err(|e| AppError::new(format!("getDisplayId result failed: {e}")))?;
            let global = env
                .new_global_ref(&obj)
                .map_err(|e| AppError::new(format!("new_global_ref failed: {e}")))?;
            Ok((global, display_id))
        })?;

        let (global, display_id) = instance;
        let assigned_package = {
            let p = &req.package;
            let installed = self.list_installed_packages(p)?;
            if installed.contains(p) {
                p.clone()
            } else {
                self.allocate_from_prefix(p, &installed)?
            }
        };

        let screen = VirtualScreen {
            display_id,
            instance: global,
            last_jpeg: None,
            width: req.width,
            height: req.height,
            dpi: req.dpi,
            last_heartbeat: Instant::now(),
            timeout_secs: req.timeout_secs,
            last_interaction: None,
            assigned_package: assigned_package.clone(),
        };
        self.screens.insert(req.name.clone(), screen);

        Ok(ScreenInfo {
            name: req.name.clone(),
            display_id,
            width: req.width,
            height: req.height,
            dpi: req.dpi,
            assigned_package,
        })
    }

    fn destroy_screen(&mut self, name: &str) -> Result<(), AppError> {
        let screen = self
            .screens
            .remove(name)
            .ok_or_else(|| AppError::not_found(format!("screen {name} not found")))?;

        self.with_env(|env| {
            let obj: &JObject = screen.instance.as_obj();
            env.call_method(obj, "release", "()V", &[]).map_err(|e| {
                if let Some(exc_msg) = get_exception_message(env) {
                    AppError::new(format!("release failed: {exc_msg}"))
                } else {
                    AppError::new(format!("release failed: {e}"))
                }
            })?;
            Ok(())
        })
    }

    fn list_screens(&self) -> Vec<ScreenInfo> {
        self.screens
            .iter()
            .map(|(name, s)| ScreenInfo {
                name: name.clone(),
                display_id: s.display_id,
                width: s.width,
                height: s.height,
                dpi: s.dpi,
                assigned_package: s.assigned_package.clone(),
            })
            .collect()
    }

    fn screen_info(&self, name: &str) -> Result<ScreenInfo, AppError> {
        let s = self.get_screen(name)?;
        Ok(ScreenInfo {
            name: name.to_string(),
            display_id: s.display_id,
            width: s.width,
            height: s.height,
            dpi: s.dpi,
            assigned_package: s.assigned_package.clone(),
        })
    }

    fn screenshot(&mut self, name: &str) -> Result<Vec<u8>, AppError> {
        let screen = self.get_screen(name)?;
        let width = screen.width as u32;
        let height = screen.height as u32;
        let instance = screen.instance.clone();

        let new_jpeg = self.with_env(|env| {
            let obj: &JObject = instance.as_obj();
            let rgba_array: JByteArray = env
                .call_method(obj, "takeScreenshotRGBA", "()[B", &[])
                .map_err(|e| {
                    if let Some(exc_msg) = get_exception_message(env) {
                        AppError::new(format!("takeScreenshotRGBA call failed: {exc_msg}"))
                    } else {
                        AppError::new(format!("takeScreenshotRGBA call failed: {e}"))
                    }
                })?
                .l()
                .map_err(|e| AppError::new(format!("takeScreenshotRGBA result failed: {e}")))?
                .into();

            if rgba_array.is_null() {
                return Ok(None);
            }

            let elements = unsafe {
                env.get_array_elements(&rgba_array, jni::objects::ReleaseMode::NoCopyBack)
                    .map_err(|e| AppError::new(format!("get array elements failed: {e}")))?
            };

            let rgba: &[u8] = unsafe {
                std::slice::from_raw_parts(elements.as_ptr() as *const u8, elements.len())
            };

            let jpeg = encode_jpeg(rgba, width, height)?;
            drop(elements);

            Ok(Some(jpeg))
        })?;

        let screen = self.get_screen_mut(name)?;
        match new_jpeg {
            Some(jpeg) => {
                screen.last_jpeg = Some(jpeg);
            }
            None if screen.last_jpeg.is_none() => {
                return Err(AppError::new("no frame available"));
            }
            None => {}
        }
        Ok(screen.last_jpeg.clone().unwrap())
    }

    fn tap(&mut self, name: &str, x: f32, y: f32) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        let instance = screen.instance.clone();
        self.with_env(|env| {
            let obj: &JObject = instance.as_obj();
            call_instance_void(
                env,
                obj,
                "injectTap",
                "(FF)V",
                &[JValue::Float(x), JValue::Float(y)],
            )
        })?;
        self.screens.get_mut(name).unwrap().last_interaction = Some(Instant::now());
        Ok(())
    }

    fn swipe(&mut self, name: &str, req: &SwipeRequest) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        let instance = screen.instance.clone();
        self.with_env(|env| {
            let obj: &JObject = instance.as_obj();
            call_instance_void(
                env,
                obj,
                "injectSwipe",
                "(FFFFJ)V",
                &[
                    JValue::Float(req.x1),
                    JValue::Float(req.y1),
                    JValue::Float(req.x2),
                    JValue::Float(req.y2),
                    JValue::Long(req.duration_ms),
                ],
            )
        })?;
        self.screens.get_mut(name).unwrap().last_interaction = Some(Instant::now());
        Ok(())
    }

    fn input_text(&mut self, name: &str, text: &str) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        let instance = screen.instance.clone();
        self.with_env(|env| {
            let obj: &JObject = instance.as_obj();
            let jtext = env
                .new_string(text)
                .map_err(|e| AppError::new(format!("new_string failed: {e}")))?;
            call_instance_void(
                env,
                obj,
                "injectText",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&jtext)],
            )
        })?;
        self.screens.get_mut(name).unwrap().last_interaction = Some(Instant::now());
        Ok(())
    }

    fn key(&mut self, name: &str, keycode: i32) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        let instance = screen.instance.clone();
        self.with_env(|env| {
            let obj: &JObject = instance.as_obj();
            call_instance_void(env, obj, "injectKey", "(I)V", &[JValue::Int(keycode)])
        })?;
        self.screens.get_mut(name).unwrap().last_interaction = Some(Instant::now());
        Ok(())
    }

    fn accessibility_tree(&mut self, name: &str) -> Result<String, AppError> {
        let screen = self.get_screen_mut(name)?;
        let display_id = screen.display_id;
        let bridge = self.a11y_bridge.clone();
        self.with_env(|env| {
            let obj: &JObject = bridge.as_obj();
            let json_obj = env
                .call_method(
                    obj,
                    "dumpDisplayJson",
                    "(I)Ljava/lang/String;",
                    &[JValue::Int(display_id)],
                )
                .map_err(|e| {
                    if let Some(exc_msg) = get_exception_message(env) {
                        AppError::new(format!("dumpDisplayJson call failed: {exc_msg}"))
                    } else {
                        AppError::new(format!("dumpDisplayJson call failed: {e}"))
                    }
                })?
                .l()
                .map_err(|e| AppError::new(format!("dumpDisplayJson result failed: {e}")))?;
            if json_obj.is_null() {
                return Err(AppError::new("dumpDisplayJson returned null"));
            }
            let json_jstr: JString = json_obj.into();
            let json: String = env
                .get_string(&json_jstr)
                .map_err(|e| AppError::new(format!("dumpDisplayJson decode failed: {e}")))?
                .into();
            Ok(json)
        })
    }

    fn launch(&mut self, name: &str) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        let display_id = screen.display_id;
        let package = &screen.assigned_package;

        let resolve = Command::new("cmd")
            .args(["package", "resolve-activity", "--brief", package])
            .output()
            .map_err(|e| AppError::new(format!("resolve-activity failed: {e}")))?;

        let resolve_out = String::from_utf8_lossy(&resolve.stdout);
        let component = resolve_out
            .lines()
            .filter(|line| line.contains('/'))
            .last()
            .map(|line| line.trim().to_string())
            .ok_or_else(|| AppError::new(format!("no activity found for {package}")))?;

        let start = Command::new("am")
            .args([
                "start",
                "--activity-clear-task",
                "--display",
                &display_id.to_string(),
                "-n",
                &component,
            ])
            .output()
            .map_err(|e| AppError::new(format!("am start failed: {e}")))?;

        if !start.status.success() {
            let stdout = String::from_utf8_lossy(&start.stdout);
            let stderr = String::from_utf8_lossy(&start.stderr);
            return Err(AppError::new(format!(
                "am start failed: {} {}",
                stdout.trim(),
                stderr.trim()
            )));
        }

        let start_stdout = String::from_utf8_lossy(&start.stdout);
        if start_stdout.contains("Activity not started") {
            return Err(AppError::new(format!(
                "am start did not launch a new task: {}",
                start_stdout.trim()
            )));
        }

        Ok(())
    }

    fn open_url(&mut self, name: &str, url: &str) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        let display_id = screen.display_id;
        let package = &screen.assigned_package;
        let start = Command::new("am")
            .args([
                "start",
                "--display",
                &display_id.to_string(),
                "-a",
                "android.intent.action.VIEW",
                "-d",
                url,
                "-n",
                &format!("{package}/.MainActivity"),
            ])
            .output()
            .map_err(|e| AppError::new(format!("am start failed: {e}")))?;
        if !start.status.success() {
            let stdout = String::from_utf8_lossy(&start.stdout);
            let stderr = String::from_utf8_lossy(&start.stderr);
            return Err(AppError::new(format!(
                "am start failed: {} {}",
                stdout.trim(),
                stderr.trim()
            )));
        }
        Ok(())
    }

    fn wait_for_idle(
        &mut self,
        name: &str,
        idle_timeout_ms: i64,
        global_timeout_ms: i64,
    ) -> Result<bool, AppError> {
        self.get_screen_mut(name)?;
        let bridge = self.a11y_bridge.clone();
        self.with_env(|env| {
            let obj: &JObject = bridge.as_obj();
            let result = env
                .call_method(
                    obj,
                    "waitForIdle",
                    "(JJ)Z",
                    &[
                        JValue::Long(idle_timeout_ms),
                        JValue::Long(global_timeout_ms),
                    ],
                )
                .map_err(|e| {
                    if let Some(exc_msg) = get_exception_message(env) {
                        AppError::new(format!("waitForIdle call failed: {exc_msg}"))
                    } else {
                        AppError::new(format!("waitForIdle call failed: {e}"))
                    }
                })?;
            Ok(result.z().unwrap_or(false))
        })
    }

    fn heartbeat(&mut self, name: &str) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        screen.last_heartbeat = Instant::now();
        Ok(())
    }

    fn reap_dead_screens(&mut self) {
        let dead: Vec<String> = self
            .screens
            .iter()
            .filter(|(_, s)| {
                s.last_heartbeat.elapsed() > std::time::Duration::from_secs(s.timeout_secs)
            })
            .map(|(name, _)| name.clone())
            .collect();

        for name in dead {
            if let Some(screen) = self.screens.remove(&name) {
                tracing::info!(name = %name, display_id = screen.display_id, "reaping dead screen (timeout {}s)", screen.timeout_secs);
                let _ = self.with_env(|env| {
                    let obj: &JObject = screen.instance.as_obj();
                    let _ = env.call_method(obj, "release", "()V", &[]);
                    if env.exception_check().unwrap_or(false) {
                        env.exception_clear().ok();
                    }
                    Ok(())
                });
            }
        }
    }

    fn stop(&mut self, name: &str) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        let package = &screen.assigned_package;

        let status = Command::new("am")
            .args(["force-stop", package])
            .status()
            .map_err(|e| AppError::new(format!("am force-stop failed: {e}")))?;

        if !status.success() {
            return Err(AppError::new(format!("am force-stop failed for {package}")));
        }

        Ok(())
    }

    fn reset(&mut self, name: &str) -> Result<(), AppError> {
        let screen = self.get_screen_mut(name)?;
        let package = &screen.assigned_package;

        let output = Command::new("pm")
            .args(["clear", package])
            .output()
            .map_err(|e| AppError::new(format!("pm clear failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::new(format!(
                "pm clear failed for {package}: {}",
                stderr.trim()
            )));
        }

        Ok(())
    }
}

impl ServerState {
    fn list_installed_packages(&self, filter: &str) -> Result<std::collections::HashSet<String>, AppError> {
        let output = Command::new("pm")
            .args(["list", "packages", filter])
            .output()
            .map_err(|e| AppError::new(format!("pm list packages failed: {e}")))?;
        if !output.status.success() {
            return Err(AppError::new("pm list packages failed"));
        }
        Ok(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|l| l.strip_prefix("package:").map(|s| s.trim().to_string()))
                .collect(),
        )
    }

    fn allocate_from_prefix(
        &self,
        prefix: &str,
        installed_input: &std::collections::HashSet<String>,
    ) -> Result<String, AppError> {
        let assigned: std::collections::HashSet<String> = self
            .screens
            .values()
            .map(|s| s.assigned_package.clone())
            .collect();
        let mut candidates: Vec<String> = installed_input
            .iter()
            .filter(|pkg| pkg.starts_with(prefix))
            .cloned()
            .collect();
        candidates.sort();
        for candidate in candidates {
            if !assigned.contains(&candidate) {
                return Ok(candidate);
            }
        }
        Err(AppError::new("no free package available with given prefix"))
    }
}

fn format_exception(env: &mut JNIEnv, exc: &JObject) -> String {
    // Use Throwable.printStackTrace(PrintWriter) to get the full trace including cause chain
    let mut try_format = || -> Option<String> {
        let sw_class = env.find_class("java/io/StringWriter").ok()?;
        let sw = env.new_object(&sw_class, "()V", &[]).ok()?;
        let pw_class = env.find_class("java/io/PrintWriter").ok()?;
        let pw = env
            .new_object(&pw_class, "(Ljava/io/Writer;)V", &[JValue::Object(&sw)])
            .ok()?;
        env.call_method(
            exc,
            "printStackTrace",
            "(Ljava/io/PrintWriter;)V",
            &[JValue::Object(&pw)],
        )
        .ok()?;
        let result = env
            .call_method(&sw, "toString", "()Ljava/lang/String;", &[])
            .ok()?
            .l()
            .ok()?;
        let jstr: JString = result.into();
        env.get_string(&jstr).ok().map(|s| s.into())
    };
    try_format().unwrap_or_else(|| "<failed to format exception>".into())
}

fn get_exception_message(env: &mut JNIEnv) -> Option<String> {
    if !env.exception_check().unwrap_or(false) {
        return None;
    }
    let exc = env.exception_occurred().ok()?;
    env.exception_clear().ok();
    Some(format_exception(env, &exc))
}

fn call_instance_void(
    env: &mut JNIEnv,
    obj: &JObject,
    method: &str,
    sig: &str,
    args: &[JValue],
) -> Result<(), AppError> {
    env.call_method(obj, method, sig, args).map_err(|e| {
        if let Some(exc_msg) = get_exception_message(env) {
            AppError::new(format!("{method} call failed: {exc_msg}"))
        } else {
            AppError::new(format!("{method} call failed: {e}"))
        }
    })?;
    Ok(())
}

fn auto_wait_for_idle(guard: &mut ServerState, name: &str) -> Result<u64, AppError> {
    if let Some(last_interaction) = guard.get_screen(name)?.last_interaction {
        let elapsed = last_interaction.elapsed();
        let global_timeout = std::time::Duration::from_millis(2500).saturating_sub(elapsed);
        if !global_timeout.is_zero() {
            let wait_start = Instant::now();
            guard.wait_for_idle(name, 750, global_timeout.as_millis() as i64)?;
            return Ok(wait_start.elapsed().as_millis() as u64);
        }
    }
    Ok(0)
}

// --- Route handlers ---

async fn create_screen(
    State(state): State<AppState>,
    Json(req): Json<CreateScreenRequest>,
) -> Result<Json<ScreenInfo>, AppError> {
    let info = state.lock().await.create_screen(&req)?;
    Ok(Json(info))
}

async fn delete_screen(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    state.lock().await.destroy_screen(&name)?;
    Ok(StatusCode::OK)
}

async fn list_screens(State(state): State<AppState>) -> Json<Vec<ScreenInfo>> {
    Json(state.lock().await.list_screens())
}

async fn screen_info(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ScreenInfo>, AppError> {
    let info = state.lock().await.screen_info(&name)?;
    Ok(Json(info))
}

async fn screenshot(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<NoWaitQuery>,
) -> Result<Response, AppError> {
    let mut guard = state.lock().await;
    let waited_ms = if query.no_wait {
        0
    } else {
        auto_wait_for_idle(&mut guard, &name)?
    };
    let jpeg = guard.screenshot(&name)?;
    let mut response = ([(header::CONTENT_TYPE, "image/jpeg")], jpeg).into_response();
    response
        .headers_mut()
        .insert("X-Wait-Ms", waited_ms.to_string().parse().unwrap());
    Ok(response)
}

async fn a11y(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<NoWaitQuery>,
) -> Result<Response, AppError> {
    let mut guard = state.lock().await;
    let waited_ms = if query.no_wait {
        0
    } else {
        auto_wait_for_idle(&mut guard, &name)?
    };
    let json = guard.accessibility_tree(&name)?;
    let mut response = ([(header::CONTENT_TYPE, "application/json")], json).into_response();
    response
        .headers_mut()
        .insert("X-Wait-Ms", waited_ms.to_string().parse().unwrap());
    Ok(response)
}

async fn tap(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<NoWaitQuery>,
    Json(req): Json<TapRequest>,
) -> Result<Response, AppError> {
    let mut guard = state.lock().await;
    guard.tap(&name, req.x, req.y)?;
    let waited_ms = if query.no_wait {
        0
    } else {
        auto_wait_for_idle(&mut guard, &name)?
    };
    let mut response = StatusCode::OK.into_response();
    response
        .headers_mut()
        .insert("X-Wait-Ms", waited_ms.to_string().parse().unwrap());
    Ok(response)
}

async fn swipe(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<SwipeRequest>,
) -> Result<StatusCode, AppError> {
    state.lock().await.swipe(&name, &req)?;
    Ok(StatusCode::OK)
}

async fn type_text(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<TypeRequest>,
) -> Result<StatusCode, AppError> {
    state.lock().await.input_text(&name, &req.text)?;
    Ok(StatusCode::OK)
}

async fn key(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<KeyRequest>,
) -> Result<StatusCode, AppError> {
    state.lock().await.key(&name, req.keycode)?;
    Ok(StatusCode::OK)
}

async fn launch(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<NoWaitQuery>,
) -> Result<Response, AppError> {
    let mut guard = state.lock().await;
    guard.launch(&name)?;
    let waited_ms = if query.no_wait {
        0
    } else {
        let wait_start = Instant::now();
        guard.wait_for_idle(&name, 5000, 30000)?;
        wait_start.elapsed().as_millis() as u64
    };
    let mut response = StatusCode::OK.into_response();
    response
        .headers_mut()
        .insert("X-Wait-Ms", waited_ms.to_string().parse().unwrap());
    Ok(response)
}

async fn stop(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    state.lock().await.stop(&name)?;
    Ok(StatusCode::OK)
}

async fn reset(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    state.lock().await.reset(&name)?;
    Ok(StatusCode::OK)
}

async fn heartbeat(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    state.lock().await.heartbeat(&name)?;
    Ok(StatusCode::OK)
}

async fn open_url(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<OpenUrlRequest>,
) -> Result<StatusCode, AppError> {
    state.lock().await.open_url(&name, &req.url)?;
    Ok(StatusCode::OK)
}

async fn wait_for_idle(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<WaitForIdleRequest>,
) -> Result<StatusCode, AppError> {
    state
        .lock()
        .await
        .wait_for_idle(&name, req.idle_timeout_ms, req.global_timeout_ms)?;
    Ok(StatusCode::OK)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_coordinator_Main_nativeRun(
    mut env: JNIEnv,
    _class: JClass,
    _java_args: JObjectArray,
) {
    let is_daemon = std::env::var("ANDY_DAEMON").is_ok();

    if !is_daemon {
        // Parent: spawn daemon child, print "ready", exit
        let mut cmd = Command::new("app_process");
        cmd.arg0("andy-coordinator")
            .args(["/system/bin", "com.coordinator.Main"])
            .env("ANDY_DAEMON", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn().expect("spawn daemon");
        return;
    }

    let log_file =
        std::fs::File::create("/data/local/tests/coordinator/andy.log").expect("create log file");
    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .init();

    tracing::info!(port = PORT, "starting coordinator");

    let screen_class = env
        .find_class("com/coordinator/VirtualScreen")
        .expect("find VirtualScreen class");

    let screen_class_global = env
        .new_global_ref(&screen_class)
        .expect("create global ref for VirtualScreen");

    let a11y_class = env
        .find_class("com/coordinator/AccessibilityBridge")
        .expect("find AccessibilityBridge class");
    let a11y_obj = {
        let mut obj = None;
        for attempt in 0..30 {
            match env.new_object(&a11y_class, "()V", &[]) {
                Ok(o) => {
                    obj = Some(o);
                    break;
                }
                Err(_) => {
                    if env.exception_check().unwrap_or(false) {
                        env.exception_clear().ok();
                    }
                    if attempt < 29 {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
            }
        }
        obj.expect("AccessibilityBridge constructor failed after retries")
    };
    let a11y_bridge = env
        .new_global_ref(&a11y_obj)
        .expect("create global ref for AccessibilityBridge");

    let jvm = env.get_java_vm().expect("get JavaVM");
    let state: AppState = Arc::new(tokio::sync::Mutex::new(ServerState {
        jvm,
        screen_class: screen_class_global,
        screens: HashMap::new(),
        a11y_bridge,
    }));

    let app = Router::new()
        .route("/screens", post(create_screen))
        .route("/screens/{name}", delete(delete_screen))
        .route("/debug/screens", get(list_screens))
        .route("/screens/{name}/info", get(screen_info))
        .route("/screens/{name}/screenshot", get(screenshot))
        .route("/screens/{name}/a11y", get(a11y))
        .route("/screens/{name}/tap", post(tap))
        .route("/screens/{name}/swipe", post(swipe))
        .route("/screens/{name}/type", post(type_text))
        .route("/screens/{name}/key", post(key))
        .route("/screens/{name}/launch", post(launch))
        .route("/screens/{name}/stop", post(stop))
        .route("/screens/{name}/reset", post(reset))
        .route("/screens/{name}/heartbeat", post(heartbeat))
        .route("/screens/{name}/open-url", post(open_url))
        .route("/screens/{name}/wait-for-idle", post(wait_for_idle))
        .layer(
            tower_http::compression::CompressionLayer::new()
                .zstd(true)
                .no_br()
                .no_gzip()
                .no_deflate(),
        )
        .with_state(state.clone());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    runtime.block_on(async move {
        // Spawn reaper task that checks for dead screens every 2s
        let reaper_state = state.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(std::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                reaper_state.lock().await.reap_dead_screens();
            }
        });

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], PORT));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("bind tcp listener");
        tracing::info!(port = PORT, "http api ready");
        axum::serve(listener, app).await.expect("tcp server failed");
    });
}
