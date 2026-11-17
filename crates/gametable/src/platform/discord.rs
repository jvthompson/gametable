//! Discord: Rich Presence plus the friend-invite flow (OAuth, friends list, activity invites,
//! join callback) over the Discord Social SDK, loaded at runtime. Port of
//! `src/platform/discord.lua`.
//!
//! Everything is best effort: on any platform, library or SDK failure the module stays disabled
//! and every call is a no-op (or resolves to failure), since this is decorative and must never
//! crash or block a game.
//!
//! The SDK only calls back from inside `Discord_RunCallbacks()`, pumped from [`update`] on the
//! main thread. The C callbacks just copy their data (strings and results belong to the
//! callback, which frees them as the SDK's own C++ wrapper does) into an event list; the
//! continuations run after the pump returns.
//!
//! Signatures follow the SDK's `cdiscord.h` (1.10). Unlike the original's LuaJIT declarations,
//! `Discord_String` arguments that the header passes by value are declared by value, which is
//! what the macOS and Linux calling conventions need.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::native_libraries;

/// The GameTable application on Discord.
const APPLICATION_ID: u64 = 1551391208423555102;
const PUMP_INTERVAL: f32 = 0.1;
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// `Discord_Client_Status_Ready`.
pub const STATUS_READY: i32 = 3;
const ACTIVITY_PLAYING: i32 = 0;
const RELATIONSHIP_FRIEND: i32 = 1;
const PARTY_PRIVATE: i32 = 0;
const PLATFORM_DESKTOP: i32 = 1;
const TOKEN_BEARER: i32 = 1;

/// `D.statusLabels`: a friend's `Discord_StatusType` (Invisible friends read as Offline).
pub fn status_label(status: i32) -> &'static str {
    match status {
        0 => "Online",
        1 => "Offline",
        3 => "Idle",
        4 => "Do Not Disturb",
        6 => "Streaming",
        _ => "",
    }
}

// ---------------------------------------------------------------------------------------------
// The C API
// ---------------------------------------------------------------------------------------------

/// Every SDK object handle is one pointer (`struct { void* opaque; }`).
#[repr(C)]
#[derive(Clone, Copy)]
struct Handle {
    opaque: *mut c_void,
}

impl Handle {
    fn empty() -> Handle {
        Handle { opaque: std::ptr::null_mut() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DiscordString {
    pointer: *mut u8,
    size: usize,
}

impl DiscordString {
    fn empty() -> DiscordString {
        DiscordString { pointer: std::ptr::null_mut(), size: 0 }
    }

    /// A view of `text` for the duration of one call (the SDK copies what it keeps).
    fn borrowed(text: &str) -> DiscordString {
        DiscordString { pointer: text.as_ptr().cast_mut(), size: text.len() }
    }
}

#[repr(C)]
struct HandleSpan {
    pointer: *mut Handle,
    size: usize,
}

type FreeFunction = unsafe extern "C" fn(*mut c_void);
type ResultCallback = unsafe extern "C" fn(*mut Handle, *mut c_void);
type StatusCallback = unsafe extern "C" fn(i32, i32, i32, *mut c_void);
type JoinCallback = unsafe extern "C" fn(DiscordString, *mut c_void);
type AuthorizeCallback = unsafe extern "C" fn(*mut Handle, DiscordString, DiscordString, *mut c_void);
type TokenCallback =
    unsafe extern "C" fn(*mut Handle, DiscordString, DiscordString, i32, i32, DiscordString, *mut c_void);

/// The functions used, bound from the library.
struct Sdk {
    free: unsafe extern "C" fn(*mut c_void),
    run_callbacks: unsafe extern "C" fn(),
    client_init: unsafe extern "C" fn(*mut Handle),
    client_drop: unsafe extern "C" fn(*mut Handle),
    client_set_application_id: unsafe extern "C" fn(*mut Handle, u64),
    activity_init: unsafe extern "C" fn(*mut Handle),
    activity_drop: unsafe extern "C" fn(*mut Handle),
    activity_set_type: unsafe extern "C" fn(*mut Handle, i32),
    activity_set_state: unsafe extern "C" fn(*mut Handle, *mut DiscordString),
    activity_set_details: unsafe extern "C" fn(*mut Handle, *mut DiscordString),
    activity_set_party: unsafe extern "C" fn(*mut Handle, *mut Handle),
    activity_set_secrets: unsafe extern "C" fn(*mut Handle, *mut Handle),
    activity_set_supported_platforms: unsafe extern "C" fn(*mut Handle, i32),
    client_update_rich_presence:
        unsafe extern "C" fn(*mut Handle, *mut Handle, ResultCallback, FreeFunction, *mut c_void),
    client_clear_rich_presence: unsafe extern "C" fn(*mut Handle),
    party_init: unsafe extern "C" fn(*mut Handle),
    party_drop: unsafe extern "C" fn(*mut Handle),
    party_set_id: unsafe extern "C" fn(*mut Handle, DiscordString),
    party_set_current_size: unsafe extern "C" fn(*mut Handle, i32),
    party_set_max_size: unsafe extern "C" fn(*mut Handle, i32),
    party_set_privacy: unsafe extern "C" fn(*mut Handle, i32),
    secrets_init: unsafe extern "C" fn(*mut Handle),
    secrets_drop: unsafe extern "C" fn(*mut Handle),
    secrets_set_join: unsafe extern "C" fn(*mut Handle, DiscordString),
    default_presence_scopes: unsafe extern "C" fn(*mut DiscordString),
    default_communication_scopes: unsafe extern "C" fn(*mut DiscordString),
    create_code_verifier: unsafe extern "C" fn(*mut Handle, *mut Handle),
    verifier_verifier: unsafe extern "C" fn(*mut Handle, *mut DiscordString),
    verifier_challenge: unsafe extern "C" fn(*mut Handle, *mut Handle),
    verifier_drop: unsafe extern "C" fn(*mut Handle),
    challenge_drop: unsafe extern "C" fn(*mut Handle),
    authorization_args_init: unsafe extern "C" fn(*mut Handle),
    authorization_args_drop: unsafe extern "C" fn(*mut Handle),
    authorization_args_set_client_id: unsafe extern "C" fn(*mut Handle, u64),
    authorization_args_set_scopes: unsafe extern "C" fn(*mut Handle, DiscordString),
    authorization_args_set_code_challenge: unsafe extern "C" fn(*mut Handle, *mut Handle),
    client_authorize: unsafe extern "C" fn(*mut Handle, *mut Handle, AuthorizeCallback, FreeFunction, *mut c_void),
    client_get_token: unsafe extern "C" fn(
        *mut Handle,
        u64,
        DiscordString,
        DiscordString,
        DiscordString,
        TokenCallback,
        FreeFunction,
        *mut c_void,
    ),
    client_refresh_token:
        unsafe extern "C" fn(*mut Handle, u64, DiscordString, TokenCallback, FreeFunction, *mut c_void),
    client_update_token:
        unsafe extern "C" fn(*mut Handle, i32, DiscordString, ResultCallback, FreeFunction, *mut c_void),
    client_connect: unsafe extern "C" fn(*mut Handle),
    client_set_status_changed_callback: unsafe extern "C" fn(*mut Handle, StatusCallback, FreeFunction, *mut c_void),
    client_get_relationships: unsafe extern "C" fn(*mut Handle, *mut HandleSpan),
    relationship_type: unsafe extern "C" fn(*mut Handle) -> i32,
    relationship_user: unsafe extern "C" fn(*mut Handle, *mut Handle) -> bool,
    relationship_drop: unsafe extern "C" fn(*mut Handle),
    user_id: unsafe extern "C" fn(*mut Handle) -> u64,
    user_username: unsafe extern "C" fn(*mut Handle, *mut DiscordString),
    user_display_name: unsafe extern "C" fn(*mut Handle, *mut DiscordString),
    user_status: unsafe extern "C" fn(*mut Handle) -> i32,
    user_drop: unsafe extern "C" fn(*mut Handle),
    client_register_launch_command: unsafe extern "C" fn(*mut Handle, u64, DiscordString) -> bool,
    client_send_activity_invite:
        unsafe extern "C" fn(*mut Handle, u64, DiscordString, ResultCallback, FreeFunction, *mut c_void),
    client_set_activity_join_callback: unsafe extern "C" fn(*mut Handle, JoinCallback, FreeFunction, *mut c_void),
    result_successful: unsafe extern "C" fn(*mut Handle) -> bool,
    result_error: unsafe extern "C" fn(*mut Handle, *mut DiscordString),
    result_error_code: unsafe extern "C" fn(*mut Handle) -> i32,
    result_drop: unsafe extern "C" fn(*mut Handle),
    _library: libloading::Library,
}

// The function pointers are plain C entry points, only called from the main thread.
unsafe impl Send for Sdk {}
unsafe impl Sync for Sdk {}

macro_rules! bind {
    ($library:expr, $name:literal) => {
        *$library.get(concat!($name, "\0").as_bytes()).ok()?
    };
}

fn bind_sdk(library: libloading::Library) -> Option<Sdk> {
    unsafe {
        Some(Sdk {
            free: bind!(library, "Discord_Free"),
            run_callbacks: bind!(library, "Discord_RunCallbacks"),
            client_init: bind!(library, "Discord_Client_Init"),
            client_drop: bind!(library, "Discord_Client_Drop"),
            client_set_application_id: bind!(library, "Discord_Client_SetApplicationId"),
            activity_init: bind!(library, "Discord_Activity_Init"),
            activity_drop: bind!(library, "Discord_Activity_Drop"),
            activity_set_type: bind!(library, "Discord_Activity_SetType"),
            activity_set_state: bind!(library, "Discord_Activity_SetState"),
            activity_set_details: bind!(library, "Discord_Activity_SetDetails"),
            activity_set_party: bind!(library, "Discord_Activity_SetParty"),
            activity_set_secrets: bind!(library, "Discord_Activity_SetSecrets"),
            activity_set_supported_platforms: bind!(library, "Discord_Activity_SetSupportedPlatforms"),
            client_update_rich_presence: bind!(library, "Discord_Client_UpdateRichPresence"),
            client_clear_rich_presence: bind!(library, "Discord_Client_ClearRichPresence"),
            party_init: bind!(library, "Discord_ActivityParty_Init"),
            party_drop: bind!(library, "Discord_ActivityParty_Drop"),
            party_set_id: bind!(library, "Discord_ActivityParty_SetId"),
            party_set_current_size: bind!(library, "Discord_ActivityParty_SetCurrentSize"),
            party_set_max_size: bind!(library, "Discord_ActivityParty_SetMaxSize"),
            party_set_privacy: bind!(library, "Discord_ActivityParty_SetPrivacy"),
            secrets_init: bind!(library, "Discord_ActivitySecrets_Init"),
            secrets_drop: bind!(library, "Discord_ActivitySecrets_Drop"),
            secrets_set_join: bind!(library, "Discord_ActivitySecrets_SetJoin"),
            default_presence_scopes: bind!(library, "Discord_Client_GetDefaultPresenceScopes"),
            default_communication_scopes: bind!(library, "Discord_Client_GetDefaultCommunicationScopes"),
            create_code_verifier: bind!(library, "Discord_Client_CreateAuthorizationCodeVerifier"),
            verifier_verifier: bind!(library, "Discord_AuthorizationCodeVerifier_Verifier"),
            verifier_challenge: bind!(library, "Discord_AuthorizationCodeVerifier_Challenge"),
            verifier_drop: bind!(library, "Discord_AuthorizationCodeVerifier_Drop"),
            challenge_drop: bind!(library, "Discord_AuthorizationCodeChallenge_Drop"),
            authorization_args_init: bind!(library, "Discord_AuthorizationArgs_Init"),
            authorization_args_drop: bind!(library, "Discord_AuthorizationArgs_Drop"),
            authorization_args_set_client_id: bind!(library, "Discord_AuthorizationArgs_SetClientId"),
            authorization_args_set_scopes: bind!(library, "Discord_AuthorizationArgs_SetScopes"),
            authorization_args_set_code_challenge: bind!(library, "Discord_AuthorizationArgs_SetCodeChallenge"),
            client_authorize: bind!(library, "Discord_Client_Authorize"),
            client_get_token: bind!(library, "Discord_Client_GetToken"),
            client_refresh_token: bind!(library, "Discord_Client_RefreshToken"),
            client_update_token: bind!(library, "Discord_Client_UpdateToken"),
            client_connect: bind!(library, "Discord_Client_Connect"),
            client_set_status_changed_callback: bind!(library, "Discord_Client_SetStatusChangedCallback"),
            client_get_relationships: bind!(library, "Discord_Client_GetRelationships"),
            relationship_type: bind!(library, "Discord_RelationshipHandle_DiscordRelationshipType"),
            relationship_user: bind!(library, "Discord_RelationshipHandle_User"),
            relationship_drop: bind!(library, "Discord_RelationshipHandle_Drop"),
            user_id: bind!(library, "Discord_UserHandle_Id"),
            user_username: bind!(library, "Discord_UserHandle_Username"),
            user_display_name: bind!(library, "Discord_UserHandle_DisplayName"),
            user_status: bind!(library, "Discord_UserHandle_Status"),
            user_drop: bind!(library, "Discord_UserHandle_Drop"),
            client_register_launch_command: bind!(library, "Discord_Client_RegisterLaunchCommand"),
            client_send_activity_invite: bind!(library, "Discord_Client_SendActivityInvite"),
            client_set_activity_join_callback: bind!(library, "Discord_Client_SetActivityJoinCallback"),
            result_successful: bind!(library, "Discord_ClientResult_Successful"),
            result_error: bind!(library, "Discord_ClientResult_Error"),
            result_error_code: bind!(library, "Discord_ClientResult_ErrorCode"),
            result_drop: bind!(library, "Discord_ClientResult_Drop"),
            _library: library,
        })
    }
}

/// The SDK, loaded at most once per process.
static SDK: OnceLock<Option<Sdk>> = OnceLock::new();

fn sdk() -> Option<&'static Sdk> {
    SDK.get_or_init(|| {
        native_libraries::candidates("discord_partner_sdk")
            .into_iter()
            .find_map(|candidate| unsafe { libloading::Library::new(&candidate) }.ok().and_then(bind_sdk))
    })
    .as_ref()
}

fn log(message: &str) {
    eprintln!("Discord: {message}");
}

impl Sdk {
    /// Copies a string the SDK handed over, then frees it.
    fn take_string(&self, text: DiscordString) -> String {
        if text.pointer.is_null() {
            return String::new();
        }
        let copied = if text.size == 0 {
            String::new()
        } else {
            String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(text.pointer, text.size) }).into_owned()
        };
        unsafe { (self.free)(text.pointer.cast()) };
        copied
    }

    fn out_string(&self, fill: impl FnOnce(*mut DiscordString)) -> String {
        let mut text = DiscordString::empty();
        fill(&mut text);
        self.take_string(text)
    }

    /// Reads (and logs, on failure) a callback's result, then drops it.
    fn take_result(&self, label: &str, result: *mut Handle) -> bool {
        if result.is_null() {
            return false;
        }
        let ok = unsafe { (self.result_successful)(result) };
        if !ok {
            let code = unsafe { (self.result_error_code)(result) };
            let error = self.out_string(|out| unsafe { (self.result_error)(result, out) });
            log(&format!("{label} failed (code={code}, error=\"{error}\")"));
        }
        unsafe { (self.result_drop)(result) };
        ok
    }
}

// ---------------------------------------------------------------------------------------------
// Callbacks: record what happened; the main thread acts on it after the pump.
// ---------------------------------------------------------------------------------------------

/// What a finished request delivered.
#[derive(Debug, Clone)]
enum Payload {
    Done(bool),
    Authorized { code: String, redirect_uri: String },
    Tokens { access_token: String, refresh_token: String },
}

#[derive(Debug, Clone)]
enum Event {
    Request { id: usize, payload: Option<Payload> },
    Status(i32),
    Join(String),
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

fn record(event: Event) {
    if let Ok(mut events) = EVENTS.lock() {
        events.push(event);
    }
}

unsafe extern "C" fn ignore_user_data(_: *mut c_void) {}

/// Also passed (with id 0) where the result doesn't matter: the SDK calls it without a null check.
unsafe extern "C" fn on_result(result: *mut Handle, user_data: *mut c_void) {
    let Some(sdk) = sdk() else { return };
    let id = user_data as usize;
    let ok = sdk.take_result(if id == 0 { "UpdateRichPresence" } else { "request" }, result);
    if id != 0 {
        record(Event::Request { id, payload: Some(Payload::Done(ok)) });
    }
}

unsafe extern "C" fn on_status(status: i32, error: i32, detail: i32, _: *mut c_void) {
    log(&format!("status -> {status} (error={error}, detail={detail})"));
    record(Event::Status(status));
}

unsafe extern "C" fn on_join(secret: DiscordString, _: *mut c_void) {
    let Some(sdk) = sdk() else { return };
    record(Event::Join(sdk.take_string(secret)));
}

unsafe extern "C" fn on_authorize(
    result: *mut Handle,
    code: DiscordString,
    redirect_uri: DiscordString,
    user_data: *mut c_void,
) {
    let Some(sdk) = sdk() else { return };
    let ok = sdk.take_result("Authorize", result);
    let (code, redirect_uri) = (sdk.take_string(code), sdk.take_string(redirect_uri));
    let payload = ok.then_some(Payload::Authorized { code, redirect_uri });
    record(Event::Request { id: user_data as usize, payload });
}

unsafe extern "C" fn on_token(
    result: *mut Handle,
    access_token: DiscordString,
    refresh_token: DiscordString,
    _token_type: i32,
    _expires_in: i32,
    scopes: DiscordString,
    user_data: *mut c_void,
) {
    let Some(sdk) = sdk() else { return };
    let ok = sdk.take_result("token", result);
    let (access_token, refresh_token) = (sdk.take_string(access_token), sdk.take_string(refresh_token));
    sdk.take_string(scopes);
    let payload = ok.then_some(Payload::Tokens { access_token, refresh_token });
    record(Event::Request { id: user_data as usize, payload });
}

// ---------------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------------

/// The result of an asynchronous call, filled in on the main thread: `None` while it runs.
pub type Outcome = Rc<RefCell<Option<bool>>>;

/// A hosted game's party (`party` in the original).
#[derive(Debug, Clone, PartialEq)]
pub struct Party {
    pub party_id: String,
    pub current_size: i32,
    pub max_size: i32,
    /// `host:port`.
    pub join_secret: String,
}

/// A Discord friend (`fetchInvitableFriends`).
#[derive(Debug, Clone, PartialEq)]
pub struct Friend {
    pub user_id: u64,
    pub username: String,
    pub display_name: String,
    pub status: i32,
}

type Continuation = Box<dyn FnOnce(&mut State, Option<Payload>)>;
/// What runs when a step of a flow finishes (with whether it succeeded).
type Done = Box<dyn FnOnce(&mut State, bool)>;

struct Pending {
    deadline: Instant,
    label: &'static str,
    then: Continuation,
}

struct StatusWaiter {
    deadline: Instant,
    then: Done,
}

#[derive(Clone, PartialEq)]
struct Presence {
    details: String,
    state: Option<String>,
    party: Option<Party>,
}

struct State {
    sdk: &'static Sdk,
    client: Box<Handle>,
    status: i32,
    pump_timer: f32,
    next_id: usize,
    pending: HashMap<usize, Pending>,
    status_waiters: Vec<StatusWaiter>,
    current: Option<Presence>,
    published: Option<Presence>,
    join_secrets: Vec<String>,
    /// Settings changes for the main loop to apply: `Some(token)` saves, `None` removes.
    refresh_token_changes: Vec<Option<String>>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(action: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|state| state.borrow_mut().as_mut().map(action))
}

impl State {
    fn client(&mut self) -> *mut Handle {
        &mut *self.client
    }

    fn new_request(&mut self, timeout: Duration, label: &'static str, then: Continuation) -> *mut c_void {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, Pending { deadline: Instant::now() + timeout, label, then });
        id as *mut c_void
    }

    fn publish(&mut self, presence: Presence) {
        self.current = Some(presence.clone());
        if self.published.as_ref() == Some(&presence) {
            return;
        }
        let sdk = self.sdk;
        let client = self.client();
        unsafe {
            let mut activity = Handle::empty();
            (sdk.activity_init)(&mut activity);
            (sdk.activity_set_type)(&mut activity, ACTIVITY_PLAYING);
            let mut details = DiscordString::borrowed(&presence.details);
            (sdk.activity_set_details)(&mut activity, &mut details);
            if let Some(state) = &presence.state {
                let mut state = DiscordString::borrowed(state);
                (sdk.activity_set_state)(&mut activity, &mut state);
            }
            let mut party_handle = Handle::empty();
            let mut secrets = Handle::empty();
            if let Some(party) = &presence.party {
                (sdk.party_init)(&mut party_handle);
                (sdk.party_set_id)(&mut party_handle, DiscordString::borrowed(&party.party_id));
                (sdk.party_set_current_size)(&mut party_handle, party.current_size);
                (sdk.party_set_max_size)(&mut party_handle, party.max_size);
                (sdk.party_set_privacy)(&mut party_handle, PARTY_PRIVATE);
                (sdk.activity_set_party)(&mut activity, &mut party_handle);
                (sdk.secrets_init)(&mut secrets);
                (sdk.secrets_set_join)(&mut secrets, DiscordString::borrowed(&party.join_secret));
                (sdk.activity_set_secrets)(&mut activity, &mut secrets);
                // Without this Discord shows the Join button disabled ("game not detected").
                (sdk.activity_set_supported_platforms)(&mut activity, PLATFORM_DESKTOP);
            }
            (sdk.client_update_rich_presence)(client, &mut activity, on_result, ignore_user_data, std::ptr::null_mut());
            (sdk.activity_drop)(&mut activity);
            if presence.party.is_some() {
                (sdk.party_drop)(&mut party_handle);
                (sdk.secrets_drop)(&mut secrets);
            }
        }
        self.published = Some(presence);
    }

    fn dispatch(&mut self, event: Event) {
        match event {
            Event::Status(status) => {
                self.status = status;
                if status == STATUS_READY {
                    // Presence set before Connect() is cleared once it succeeds.
                    self.published = None;
                    if let Some(current) = self.current.clone() {
                        self.publish(current);
                    }
                }
                if status == STATUS_READY {
                    for waiter in std::mem::take(&mut self.status_waiters) {
                        (waiter.then)(self, true);
                    }
                }
            }
            Event::Join(secret) => {
                log(&format!("activity join received ({secret})"));
                if !secret.is_empty() {
                    self.join_secrets.push(secret);
                }
            }
            Event::Request { id, payload } => {
                if let Some(pending) = self.pending.remove(&id) {
                    (pending.then)(self, payload);
                }
            }
        }
    }

    fn pump(&mut self) {
        unsafe { (self.sdk.run_callbacks)() };
        let events = EVENTS.lock().map(|mut events| std::mem::take(&mut *events)).unwrap_or_default();
        for event in events {
            self.dispatch(event);
        }
    }

    fn update_token(&mut self, access_token: &str, done: Done) {
        let user_data = self.new_request(
            Duration::from_secs(10),
            "UpdateToken",
            Box::new(move |state, payload| done(state, matches!(payload, Some(Payload::Done(true))))),
        );
        let client = self.client();
        unsafe {
            (self.sdk.client_update_token)(
                client,
                TOKEN_BEARER,
                DiscordString::borrowed(access_token),
                on_result,
                ignore_user_data,
                user_data,
            )
        };
    }

    fn connect_and_await_ready(&mut self, done: Done) {
        if self.status == STATUS_READY {
            return done(self, true);
        }
        self.status_waiters.push(StatusWaiter { deadline: Instant::now() + READY_TIMEOUT, then: done });
        let client = self.client();
        unsafe { (self.sdk.client_connect)(client) };
    }

    fn refresh_token(&mut self, token: &str, done: Done) {
        let user_data = self.new_request(
            Duration::from_secs(10),
            "RefreshToken",
            Box::new(move |state, payload| match payload {
                Some(Payload::Tokens { access_token, refresh_token }) => {
                    log("RefreshToken succeeded, saving new refresh token");
                    state.refresh_token_changes.push(Some(refresh_token));
                    state.update_token(&access_token, done);
                }
                _ => done(state, false),
            }),
        );
        let client = self.client();
        unsafe {
            (self.sdk.client_refresh_token)(
                client,
                APPLICATION_ID,
                DiscordString::borrowed(token),
                on_token,
                ignore_user_data,
                user_data,
            )
        };
    }

    fn authorize_from_scratch(&mut self, done: Done) {
        let sdk = self.sdk;
        let client = self.client();
        let mut verifier = Box::new(Handle::empty());
        let mut challenge = Box::new(Handle::empty());
        let mut arguments = Box::new(Handle::empty());
        // The boxes move into `cleanup` below; their contents stay put.
        let arguments_pointer: *mut Handle = &mut *arguments;
        let code_verifier;
        unsafe {
            (sdk.create_code_verifier)(client, &mut *verifier);
            (sdk.verifier_challenge)(&mut *verifier, &mut *challenge);
            code_verifier = sdk.out_string(|out| (sdk.verifier_verifier)(&mut *verifier, out));
            let presence = sdk.out_string(|out| (sdk.default_presence_scopes)(out));
            let communication = sdk.out_string(|out| (sdk.default_communication_scopes)(out));
            let scopes = format!("{presence} {communication}").trim().to_string();
            (sdk.authorization_args_init)(&mut *arguments);
            (sdk.authorization_args_set_client_id)(&mut *arguments, APPLICATION_ID);
            (sdk.authorization_args_set_scopes)(&mut *arguments, DiscordString::borrowed(&scopes));
            (sdk.authorization_args_set_code_challenge)(&mut *arguments, &mut *challenge);
        }
        let cleanup = move || {
            let (mut arguments, mut challenge, mut verifier) = (arguments, challenge, verifier);
            unsafe {
                (sdk.authorization_args_drop)(&mut *arguments);
                (sdk.challenge_drop)(&mut *challenge);
                (sdk.verifier_drop)(&mut *verifier);
            }
        };
        // The user needs time on the consent screen.
        let user_data = self.new_request(
            Duration::from_secs(300),
            "Authorize",
            Box::new(move |state, payload| {
                let Some(Payload::Authorized { code, redirect_uri }) = payload else {
                    cleanup();
                    return done(state, false);
                };
                let token_user_data = state.new_request(
                    Duration::from_secs(15),
                    "GetToken",
                    Box::new(move |state, payload| {
                        cleanup();
                        let Some(Payload::Tokens { access_token, refresh_token }) = payload else {
                            return done(state, false);
                        };
                        log("GetToken succeeded, saving refresh token");
                        state.refresh_token_changes.push(Some(refresh_token));
                        state.update_token(&access_token, done);
                    }),
                );
                let client = state.client();
                unsafe {
                    (state.sdk.client_get_token)(
                        client,
                        APPLICATION_ID,
                        DiscordString::borrowed(&code),
                        DiscordString::borrowed(&code_verifier),
                        DiscordString::borrowed(&redirect_uri),
                        on_token,
                        ignore_user_data,
                        token_user_data,
                    )
                };
            }),
        );
        // SAFETY: the arguments box is owned by `cleanup`, held by the pending request, so it
        // outlives this call; it is dropped when the flow ends.
        unsafe { (sdk.client_authorize)(client, arguments_pointer, on_authorize, ignore_user_data, user_data) };
    }

    fn ensure_authorized_and_connected(&mut self, saved_refresh_token: Option<String>, outcome: Outcome) {
        let finish = {
            let outcome = outcome.clone();
            move |_: &mut State, ok: bool| *outcome.borrow_mut() = Some(ok)
        };
        if self.status == STATUS_READY {
            return finish(self, true);
        }
        let after_token = move |state: &mut State, ok: bool| {
            if !ok {
                return finish(state, false);
            }
            state.connect_and_await_ready(Box::new(finish));
        };
        match saved_refresh_token.filter(|token| !token.is_empty()) {
            Some(token) => self.refresh_token(
                &token,
                Box::new(move |state, ok| {
                    if ok {
                        return after_token(state, true);
                    }
                    state.refresh_token_changes.push(None);
                    state.authorize_from_scratch(Box::new(after_token));
                }),
            ),
            None => self.authorize_from_scratch(Box::new(after_token)),
        }
    }

    fn fetch_invitable_friends(&mut self) -> Vec<Friend> {
        let sdk = self.sdk;
        let client = self.client();
        let mut friends = Vec::new();
        unsafe {
            let mut span = HandleSpan { pointer: std::ptr::null_mut(), size: 0 };
            (sdk.client_get_relationships)(client, &mut span);
            for index in 0..span.size {
                let relationship = span.pointer.add(index);
                if (sdk.relationship_type)(relationship) == RELATIONSHIP_FRIEND {
                    let mut user = Handle::empty();
                    if (sdk.relationship_user)(relationship, &mut user) {
                        let username = sdk.out_string(|out| (sdk.user_username)(&mut user, out));
                        let display_name = sdk.out_string(|out| (sdk.user_display_name)(&mut user, out));
                        friends.push(Friend {
                            user_id: (sdk.user_id)(&mut user),
                            display_name: if display_name.is_empty() { username.clone() } else { display_name },
                            username,
                            status: (sdk.user_status)(&mut user),
                        });
                        (sdk.user_drop)(&mut user);
                    }
                }
                (sdk.relationship_drop)(relationship);
            }
            if !span.pointer.is_null() {
                (sdk.free)(span.pointer.cast());
            }
        }
        friends
    }
}

// ---------------------------------------------------------------------------------------------
// The module's API (`D.*`), used from the main thread
// ---------------------------------------------------------------------------------------------

/// `D.init()`: loads the SDK and starts publishing presence. Never fails loudly.
pub fn init() {
    if is_enabled() {
        return;
    }
    let Some(sdk) = sdk() else {
        log(&format!(
            "init failed, disabling ({} not found)",
            native_libraries::display_file_name("discord_partner_sdk")
        ));
        return;
    };
    let mut client = Box::new(Handle::empty());
    unsafe {
        (sdk.client_init)(&mut *client);
        (sdk.client_set_application_id)(&mut *client, APPLICATION_ID);
    }
    let mut state = State {
        sdk,
        client,
        status: -1,
        pump_timer: 0.0,
        next_id: 1,
        pending: HashMap::new(),
        status_waiters: Vec::new(),
        current: None,
        published: None,
        join_secrets: Vec::new(),
        refresh_token_changes: Vec::new(),
    };
    // Launch registration and the join callback are live from startup: this user may be the one
    // being invited.
    let client = state.client();
    unsafe {
        let registered = (sdk.client_register_launch_command)(client, APPLICATION_ID, DiscordString::borrowed(""));
        log(&format!("RegisterLaunchCommand -> {registered}"));
        (sdk.client_set_status_changed_callback)(client, on_status, ignore_user_data, std::ptr::null_mut());
        (sdk.client_set_activity_join_callback)(client, on_join, ignore_user_data, std::ptr::null_mut());
    }
    state.publish(idle_presence());
    STATE.with(|slot| *slot.borrow_mut() = Some(state));
}

pub fn is_enabled() -> bool {
    STATE.with(|state| state.borrow().is_some())
}

pub fn is_ready() -> bool {
    with_state(|state| state.status == STATUS_READY).unwrap_or(false)
}

/// `D.update(dt)`: pumps the SDK's callbacks, expires requests, and hands back the refresh-token
/// changes to save (`Some` saves, `None` removes).
pub fn update(dt: f32) -> Vec<Option<String>> {
    with_state(|state| {
        state.pump_timer += dt;
        if state.pump_timer >= PUMP_INTERVAL {
            state.pump_timer = 0.0;
            state.pump();
        }
        let now = Instant::now();
        let expired: Vec<usize> =
            state.pending.iter().filter(|(_, pending)| now > pending.deadline).map(|(id, _)| *id).collect();
        for id in expired {
            if let Some(pending) = state.pending.remove(&id) {
                log(&format!("{} timed out", pending.label));
                (pending.then)(state, None);
            }
        }
        let (expired, waiting): (Vec<StatusWaiter>, Vec<StatusWaiter>) =
            std::mem::take(&mut state.status_waiters).into_iter().partition(|waiter| now > waiter.deadline);
        state.status_waiters = waiting;
        for waiter in expired {
            log(&format!("never reached status {STATUS_READY}"));
            (waiter.then)(state, false);
        }
        std::mem::take(&mut state.refresh_token_changes)
    })
    .unwrap_or_default()
}

/// The join secrets of invites accepted since the last call (`D.onJoinSecret`).
pub fn take_join_secrets() -> Vec<String> {
    with_state(|state| std::mem::take(&mut state.join_secrets)).unwrap_or_default()
}

/// `D.dispose()`: clears presence and drops the client (best effort; Discord clears presence on
/// its own when the process goes away).
pub fn dispose() {
    let Some(mut state) = STATE.with(|slot| slot.borrow_mut().take()) else { return };
    let client = state.client();
    unsafe {
        (state.sdk.client_clear_rich_presence)(client);
        (state.sdk.client_drop)(client);
    }
}

fn idle_presence() -> Presence {
    Presence { details: "In the menus".to_string(), state: None, party: None }
}

/// `D.publish(details, state, party)`; repeats of the current presence are ignored.
pub fn publish(details: &str, state: Option<&str>, party: Option<Party>) {
    let presence = Presence { details: details.to_string(), state: state.map(str::to_string), party };
    with_state(|discord| discord.publish(presence));
}

/// `D.setPlaying`: "Playing <game>" / "<n>[/<max>] players".
pub fn set_playing(game_name: &str, player_count: usize, max_players: Option<usize>, party: Option<Party>) {
    let state = match max_players {
        Some(max) => format!("{player_count}/{max} players"),
        None => format!("{player_count} players"),
    };
    publish(&format!("Playing {game_name}"), Some(&state), party);
}

/// `D.setIdle`.
pub fn set_idle() {
    with_state(|discord| discord.publish(idle_presence()));
}

/// `D.ensureAuthorizedAndConnected`: the first time the host opens Invite mode. A saved refresh
/// token first (no consent screen), else a full Authorize in the browser.
pub fn ensure_authorized_and_connected(saved_refresh_token: Option<String>) -> Outcome {
    let outcome: Outcome = Rc::new(RefCell::new(None));
    if with_state(|state| state.ensure_authorized_and_connected(saved_refresh_token, outcome.clone())).is_none() {
        *outcome.borrow_mut() = Some(false);
    }
    outcome
}

/// `D.fetchInvitableFriends`: every real friend, whatever their status (an Invisible friend
/// reads as Offline, so filtering would hide reachable people).
pub fn fetch_invitable_friends() -> Vec<Friend> {
    with_state(|state| if state.status == STATUS_READY { state.fetch_invitable_friends() } else { Vec::new() })
        .unwrap_or_default()
}

/// `D.sendGameInvite`: an activity invite carrying the host's presence (its join secret is
/// `ip:port`).
pub fn send_game_invite(friend_user_id: u64) -> Outcome {
    let outcome: Outcome = Rc::new(RefCell::new(None));
    let sent = with_state(|state| {
        if state.status != STATUS_READY {
            return false;
        }
        let result = outcome.clone();
        let user_data = state.new_request(
            Duration::from_secs(10),
            "SendActivityInvite",
            Box::new(move |_, payload| *result.borrow_mut() = Some(matches!(payload, Some(Payload::Done(true))))),
        );
        let client = state.client();
        unsafe {
            (state.sdk.client_send_activity_invite)(
                client,
                friend_user_id,
                DiscordString::borrowed("Join my GameTable game!"),
                on_result,
                ignore_user_data,
                user_data,
            )
        };
        true
    });
    if sent != Some(true) {
        *outcome.borrow_mut() = Some(false);
    }
    outcome
}

/// `D.parseJoinSecret`: `host:port` -> (host, port).
pub fn parse_join_secret(secret: &str) -> Option<(String, u16)> {
    let (host, port) = secret.rsplit_once(':')?;
    if host.is_empty() || host.contains(':') || port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_secrets_and_labels() {
        assert_eq!(parse_join_secret("203.0.113.5:51234"), Some(("203.0.113.5".to_string(), 51234)));
        assert_eq!(parse_join_secret("host:"), None);
        assert_eq!(parse_join_secret(":5"), None);
        assert_eq!(parse_join_secret("a:b:5"), None);
        assert_eq!(parse_join_secret("host:12x"), None);
        assert_eq!(status_label(4), "Do Not Disturb");
        assert_eq!(status_label(5), "");
    }

    #[test]
    fn calls_without_the_sdk_are_harmless() {
        assert!(!is_enabled());
        assert!(update(1.0).is_empty());
        set_playing("Lorcana", 2, Some(4), None);
        set_idle();
        assert_eq!(*ensure_authorized_and_connected(None).borrow(), Some(false));
        assert_eq!(*send_game_invite(1).borrow(), Some(false));
        assert!(fetch_invitable_friends().is_empty());
        assert!(take_join_secrets().is_empty());
        dispose();
    }

    #[test]
    fn the_bundled_sdk_binds_every_function() {
        // Development builds find the SDK in runtime/<platform>/; every symbol must be present.
        if native_libraries::candidates("discord_partner_sdk").iter().any(|path| path.is_absolute()) {
            assert!(sdk().is_some(), "the bundled Discord SDK is missing a function");
        }
    }
}
