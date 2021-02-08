//! Primitives for listening on TCP and forwarding the data in incoming connections
//! to UDP.

use err_context::{BoxedErrorExt as _, ResultExt as _};
use std::net::{IpAddr, SocketAddr};
use structopt::StructOpt;
use tokio::net::{TcpSocket, TcpStream, UdpSocket};

#[derive(Debug, StructOpt)]
pub struct Options {
    /// The IP and TCP port to listen to for incoming traffic from udp2tcp.
    pub tcp_listen_addr: SocketAddr,

    /// The IP and UDP port to forward all traffic to.
    pub udp_forward_addr: SocketAddr,

    #[structopt(long = "udp-bind", default_value = "0.0.0.0")]
    pub udp_bind_ip: IpAddr,

    #[structopt(flatten)]
    pub tcp_options: crate::tcp_options::TcpOptions,
}

pub async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let tcp_socket = match options.tcp_listen_addr {
        SocketAddr::V4(..) => TcpSocket::new_v4(),
        SocketAddr::V6(..) => TcpSocket::new_v6(),
    }
    .context("Failed to create new TCP socket")?;
    crate::tcp_options::apply(&tcp_socket, &options.tcp_options)?;
    tcp_socket
        .set_reuseaddr(true)
        .context("Failed to set SO_REUSEADDR on TCP socket")?;
    tcp_socket
        .bind(options.tcp_listen_addr)
        .with_context(|_| format!("Failed to bind TCP socket to {}", options.tcp_listen_addr))?;
    let tcp_listener = tcp_socket.listen(1024)?;
    log::info!("Listening on {}/TCP", tcp_listener.local_addr().unwrap());

    loop {
        match tcp_listener.accept().await {
            Ok((tcp_stream, tcp_peer_addr)) => {
                log::debug!("Incoming connection from {}/TCP", tcp_peer_addr);

                let udp_bind_ip = options.udp_bind_ip;
                let udp_forward_addr = options.udp_forward_addr;
                tokio::spawn(async move {
                    if let Err(error) =
                        process_socket(tcp_stream, tcp_peer_addr, udp_bind_ip, udp_forward_addr)
                            .await
                    {
                        log::error!("Error: {}", error.display("\nCaused by: "));
                    }
                });
            }
            Err(error) => log::error!("Error when accepting incoming TCP connection: {}", error),
        }
    }
}

async fn process_socket(
    tcp_stream: TcpStream,
    tcp_peer_addr: SocketAddr,
    udp_bind_ip: IpAddr,
    udp_peer_addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let udp_bind_addr = SocketAddr::new(udp_bind_ip, 0);

    let udp_socket = UdpSocket::bind(udp_bind_addr)
        .await
        .with_context(|_| format!("Failed to bind UDP socket to {}", udp_bind_addr))?;
    udp_socket
        .connect(udp_peer_addr)
        .await
        .with_context(|_| format!("Failed to connect UDP socket to {}", udp_peer_addr))?;
    log::debug!(
        "UDP socket bound to {} and connected to {}",
        udp_socket
            .local_addr()
            .context("Failed getting local address for UDP socket")?,
        udp_peer_addr
    );

    crate::forward_traffic::process_udp_over_tcp(udp_socket, tcp_stream).await;
    log::trace!(
        "Closing forwarding for {}/TCP <-> {}/UDP",
        tcp_peer_addr,
        udp_peer_addr
    );
    Ok(())
}
