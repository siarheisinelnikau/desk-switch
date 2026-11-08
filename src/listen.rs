//! Native data listeners accept both address families without changing concrete
//! interface binds or the authenticated protocols layered on the TCP stream.
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};

pub fn tcp(address: SocketAddr) -> io::Result<tokio::net::TcpListener> {
    let socket = if address.ip().is_unspecified() {
        bind_socket(
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, address.port())),
            true,
        )
        .or_else(|error| {
            if ipv6_unavailable(&error) {
                bind_socket(
                    SocketAddr::from((Ipv4Addr::UNSPECIFIED, address.port())),
                    false,
                )
            } else {
                Err(error)
            }
        })?
    } else {
        bind_socket(address, false)?
    };
    socket.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(socket.into())
}

fn bind_socket(address: SocketAddr, dual_stack: bool) -> io::Result<Socket> {
    let socket = Socket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    )?;
    if dual_stack {
        socket.set_only_v6(false)?;
    }
    // Match the standard listener's restart behavior on Unix. Windows must not
    // reuse an in-use native service port, which could split authenticated peers.
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(16)?;
    Ok(socket)
}

pub(crate) fn ipv6_unavailable(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::AddrNotAvailable
    ) {
        return true;
    }
    #[cfg(windows)]
    const UNAVAILABLE: &[i32] = &[10042, 10043, 10047];
    #[cfg(target_os = "linux")]
    const UNAVAILABLE: &[i32] = &[92, 93, 97];
    #[cfg(target_os = "macos")]
    const UNAVAILABLE: &[i32] = &[42, 43, 47];
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    const UNAVAILABLE: &[i32] = &[];
    error
        .raw_os_error()
        .is_some_and(|code| UNAVAILABLE.contains(&code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Identity, NOISE},
        transport::Pipe,
        wire::Message,
    };
    use std::time::Duration;
    use tokio::{net::TcpStream, time::timeout};

    fn identity() -> Identity {
        let pair = snow::Builder::new(NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: pair.private.try_into().unwrap(),
            public: pair.public.try_into().unwrap(),
        }
    }

    async fn authenticated_roundtrip(listener: &tokio::net::TcpListener, address: SocketAddr) {
        let source = identity();
        let target = identity();
        timeout(Duration::from_secs(3), async {
            let outgoing = async {
                let socket = TcpStream::connect(address).await.unwrap();
                let mut pipe = Pipe::connect(socket, &source, &target.public, true)
                    .await
                    .unwrap();
                pipe.send(Message::Ping(17)).await.unwrap();
                assert_eq!(
                    pipe.messages.recv().await.unwrap().unwrap(),
                    Message::Pong(17)
                );
            };
            let incoming = async {
                let socket = listener.accept().await.unwrap().0;
                let mut pipe = Pipe::connect(socket, &target, &source.public, false)
                    .await
                    .unwrap();
                assert_eq!(
                    pipe.messages.recv().await.unwrap().unwrap(),
                    Message::Ping(17)
                );
                pipe.send(Message::Pong(17)).await.unwrap();
            };
            tokio::join!(outgoing, incoming);
        })
        .await
        .expect("the authenticated native endpoint did not complete");
    }

    #[tokio::test]
    async fn wildcard_native_input_listener_authenticates_both_ipv4_and_ipv6_peers() {
        for wildcard in ["[::]:0", "0.0.0.0:0"] {
            let listener = tcp(wildcard.parse().unwrap()).unwrap();
            let local = listener.local_addr().unwrap();
            authenticated_roundtrip(
                &listener,
                SocketAddr::from((Ipv4Addr::LOCALHOST, local.port())),
            )
            .await;
            if local.is_ipv6() {
                authenticated_roundtrip(
                    &listener,
                    SocketAddr::from((Ipv6Addr::LOCALHOST, local.port())),
                )
                .await;
                println!(
                    "Native wildcard {wildcard}: authenticated IPv4 and IPv6 loopback peers on port {}.",
                    local.port()
                );
            } else {
                println!(
                    "Native wildcard {wildcard}: IPv6 unavailable; authenticated IPv4 fallback on port {}.",
                    local.port()
                );
            }
        }
    }

    #[tokio::test]
    async fn concrete_native_listener_keeps_its_requested_interface_and_port() {
        for address in [
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, 0)),
        ] {
            let listener = match tcp(address) {
                Ok(listener) => listener,
                Err(error) if address.is_ipv6() && ipv6_unavailable(&error) => continue,
                Err(error) => panic!("concrete listener failed: {error}"),
            };
            let local = listener.local_addr().unwrap();
            assert_eq!(local.ip(), address.ip());
            authenticated_roundtrip(&listener, local).await;
        }
    }

    #[tokio::test]
    async fn an_occupied_dual_stack_port_is_reported_instead_of_falling_back() {
        let listener = tcp("[::]:0".parse().unwrap()).unwrap();
        let local = listener.local_addr().unwrap();
        let error = tcp(SocketAddr::from((Ipv6Addr::UNSPECIFIED, local.port()))).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    }
}
