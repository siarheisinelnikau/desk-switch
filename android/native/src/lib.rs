use jni::{
    Env, EnvUnowned,
    errors::{Error, Result, ThrowRuntimeExAndDefault},
    objects::{JByteArray, JObject},
    sys::{jboolean, jint, jlong},
};
use snow::{
    Builder, HandshakeState, TransportState,
    params::DHChoice,
    resolvers::{CryptoResolver, DefaultResolver},
};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};
use zeroize::Zeroizing;

const KEY_SIZE: usize = 32;
const XX: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const KK: &str = "Noise_KK_25519_ChaChaPoly_BLAKE2s";
const MAX_RECORD: usize = 60 * 1024;
const MAX_HANDSHAKE_OVERHEAD: usize = 96;

enum State {
    Handshake(Box<HandshakeState>),
    Transport(TransportState),
}

struct Session {
    state: State,
    limit: usize,
}

#[derive(Default)]
struct Sessions {
    next_handle: jlong,
    live: HashMap<jlong, Session>,
}

// A call holds this lock only while using the cipher state. Socket I/O stays in Kotlin.
static SESSIONS: LazyLock<Mutex<Sessions>> = LazyLock::new(Mutex::default);

fn noise_error(message: impl std::fmt::Display) -> Error {
    Error::ParseFailed(format!("Noise: {message}"))
}

fn with_session<T>(handle: jlong, f: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
    let mut sessions = SESSIONS.lock().map_err(|_| Error::TryLock)?;
    f(sessions
        .live
        .get_mut(&handle)
        .ok_or_else(|| noise_error("session is closed"))?)
}

fn identity(env: &mut Env<'_>, existing: &JByteArray<'_>) -> Result<Zeroizing<Vec<u8>>> {
    let mut dh = DefaultResolver
        .resolve_dh(&DHChoice::Curve25519)
        .ok_or_else(|| noise_error("missing Curve25519 support"))?;
    if existing.is_null() {
        let mut rng = DefaultResolver
            .resolve_rng()
            .ok_or_else(|| noise_error("missing random generator"))?;
        dh.generate(rng.as_mut()).map_err(noise_error)?;
    } else {
        let secret = Zeroizing::new(env.convert_byte_array(existing)?);
        if secret.len() != KEY_SIZE {
            return Err(noise_error("invalid private key size"));
        }
        dh.set(&secret);
    }
    let mut keys = Zeroizing::new(Vec::with_capacity(2 * KEY_SIZE));
    keys.extend_from_slice(dh.privkey());
    keys.extend_from_slice(dh.pubkey());
    Ok(keys)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_deskswitch_remote_data_NativeNoise_identityKeys<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    existing: JByteArray<'local>,
) -> JByteArray<'local> {
    env.with_env(|env| {
        let keys = identity(env, &existing)?;
        env.byte_array_from_slice(&keys)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_deskswitch_remote_data_NativeNoise_start<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    kk: jboolean,
    private: JByteArray<'local>,
    prologue: JByteArray<'local>,
    remote: JByteArray<'local>,
    limit: jint,
) -> jlong {
    env.with_env(|env| -> Result<_> {
        let limit = usize::try_from(limit).map_err(noise_error)?;
        if !matches!(limit, 128 | MAX_RECORD) {
            return Err(noise_error("invalid record limit"));
        }
        let private = Zeroizing::new(env.convert_byte_array(&private)?);
        let prologue = env.convert_byte_array(&prologue)?;
        if private.len() != KEY_SIZE {
            return Err(noise_error("invalid private key size"));
        }
        let pattern = if kk { KK } else { XX };
        let params = pattern.parse().map_err(noise_error)?;
        let mut builder = Builder::new(params)
            .local_private_key(&private)
            .and_then(|builder| builder.prologue(&prologue))
            .map_err(noise_error)?;
        let remote_key;
        if kk {
            remote_key = env.convert_byte_array(&remote)?;
            if remote_key.len() != KEY_SIZE {
                return Err(noise_error("invalid public key size"));
            }
            builder = builder
                .remote_public_key(&remote_key)
                .map_err(noise_error)?;
        }
        let state = builder.build_initiator().map_err(noise_error)?;
        let mut sessions = SESSIONS.lock().map_err(|_| Error::TryLock)?;
        let handle = sessions
            .next_handle
            .checked_add(1)
            .ok_or_else(|| noise_error("session limit reached"))?;
        sessions.next_handle = handle;
        sessions.live.insert(
            handle,
            Session {
                state: State::Handshake(Box::new(state)),
                limit,
            },
        );
        Ok(handle)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_deskswitch_remote_data_NativeNoise_message<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    write: jboolean,
    source: JByteArray<'local>,
) -> JByteArray<'local> {
    env.with_env(|env| {
        let source = env.convert_byte_array(&source)?;
        let output = with_session(handle, |session| {
            if source.len() > session.limit {
                return Err(noise_error("record exceeds size limit"));
            }
            let mut output = vec![0; session.limit.min(source.len() + MAX_HANDSHAKE_OVERHEAD)];
            let length = match (&mut session.state, write) {
                (State::Handshake(state), true) => state.write_message(&source, &mut output),
                (State::Handshake(state), false) => state.read_message(&source, &mut output),
                (State::Transport(state), true) => state.write_message(&source, &mut output),
                (State::Transport(state), false) => state.read_message(&source, &mut output),
            }
            .map_err(noise_error)?;
            output.truncate(length);
            Ok(output)
        })?;
        env.byte_array_from_slice(&output)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_deskswitch_remote_data_NativeNoise_finish<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> JByteArray<'local> {
    env.with_env(|env| {
        let mut sessions = SESSIONS.lock().map_err(|_| Error::TryLock)?;
        let mut session = sessions
            .live
            .remove(&handle)
            .ok_or_else(|| noise_error("session is closed"))?;
        let State::Handshake(state) = session.state else {
            return Err(noise_error("handshake already finished"));
        };
        let remote = state
            .get_remote_static()
            .ok_or_else(|| noise_error("missing remote static key"))?;
        let mut info = [0; 2 * KEY_SIZE];
        info[..KEY_SIZE].copy_from_slice(remote);
        info[KEY_SIZE..].copy_from_slice(state.get_handshake_hash());
        session.state = State::Transport(state.into_transport_mode().map_err(noise_error)?);
        sessions.live.insert(handle, session);
        drop(sessions);
        env.byte_array_from_slice(&info)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_deskswitch_remote_data_NativeNoise_release<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
) {
    env.with_env(|_| -> Result<()> {
        SESSIONS
            .lock()
            .map_err(|_| Error::TryLock)?
            .live
            .remove(&handle);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
