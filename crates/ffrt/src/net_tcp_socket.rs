//! Pre-bind TCP socket configuration for OpenHarmony.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::time::Duration;

use crate::net::{TcpListener, TcpStream};

fn socket_address(address: SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    let mut storage = unsafe { std::mem::zeroed::<libc::sockaddr_storage>() };
    match address {
        SocketAddr::V4(address) => {
            let raw = unsafe {
                &mut *(&mut storage as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in>()
            };
            raw.sin_family = libc::AF_INET as _;
            raw.sin_port = address.port().to_be();
            raw.sin_addr = libc::in_addr {
                s_addr: u32::from_ne_bytes(address.ip().octets()),
            };
            (
                storage,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        }
        SocketAddr::V6(address) => {
            let raw = unsafe {
                &mut *(&mut storage as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in6>()
            };
            raw.sin6_family = libc::AF_INET6 as _;
            raw.sin6_port = address.port().to_be();
            raw.sin6_flowinfo = address.flowinfo();
            raw.sin6_addr = libc::in6_addr {
                s6_addr: address.ip().octets(),
            };
            raw.sin6_scope_id = address.scope_id();
            (
                storage,
                std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
            )
        }
    }
}

fn decode_address(storage: &libc::sockaddr_storage) -> io::Result<SocketAddr> {
    match storage.ss_family as libc::c_int {
        libc::AF_INET => {
            let raw =
                unsafe { &*(storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
            Ok(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::from(raw.sin_addr.s_addr.to_ne_bytes()),
                u16::from_be(raw.sin_port),
            )))
        }
        libc::AF_INET6 => {
            let raw = unsafe {
                &*(storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>()
            };
            Ok(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(raw.sin6_addr.s6_addr),
                u16::from_be(raw.sin6_port),
                raw.sin6_flowinfo,
                raw.sin6_scope_id,
            )))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "socket returned a non-IP address",
        )),
    }
}

fn new_socket(domain: libc::c_int) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(domain, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let status = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if status < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, status | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    if descriptor < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, descriptor | libc::FD_CLOEXEC) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

fn set_bool(fd: RawFd, level: libc::c_int, name: libc::c_int, value: bool) -> io::Result<()> {
    let value = libc::c_int::from(value);
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&value as *const libc::c_int).cast(),
            std::mem::size_of_val(&value) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn get_int(fd: RawFd, level: libc::c_int, name: libc::c_int) -> io::Result<libc::c_int> {
    let mut value = 0;
    let mut length = std::mem::size_of_val(&value) as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            level,
            name,
            (&mut value as *mut libc::c_int).cast(),
            &mut length,
        )
    };
    if result == 0 {
        Ok(value)
    } else {
        Err(io::Error::last_os_error())
    }
}

fn set_int(fd: RawFd, level: libc::c_int, name: libc::c_int, value: u32) -> io::Result<()> {
    let value = libc::c_int::try_from(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket value is too large"))?;
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&value as *const libc::c_int).cast(),
            std::mem::size_of_val(&value) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_env = "ohos")]
fn socket_device(fd: RawFd) -> io::Result<Option<Vec<u8>>> {
    let mut device = vec![0u8; libc::IFNAMSIZ];
    let mut length = device.len() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            device.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    device.truncate(length as usize);
    if device.last() == Some(&0) {
        device.pop();
    }
    Ok((!device.is_empty()).then_some(device))
}

#[cfg(target_env = "ohos")]
fn bind_socket_device(fd: RawFd, interface: Option<&[u8]>) -> io::Result<()> {
    let mut interface = interface.unwrap_or_default().to_vec();
    if interface.contains(&0) || interface.len() >= libc::IFNAMSIZ {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid network interface name",
        ));
    }
    if !interface.is_empty() {
        interface.push(0);
    }
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            interface.as_ptr().cast(),
            interface.len() as libc::socklen_t,
        )
    };
    (result == 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
}

/// A TCP socket that can be configured before binding or connecting.
pub struct TcpSocket {
    fd: OwnedFd,
}

impl TcpSocket {
    pub fn new_v4() -> io::Result<Self> {
        Ok(Self {
            fd: new_socket(libc::AF_INET)?,
        })
    }

    pub fn new_v6() -> io::Result<Self> {
        Ok(Self {
            fd: new_socket(libc::AF_INET6)?,
        })
    }

    pub fn set_keepalive(&self, keepalive: bool) -> io::Result<()> {
        set_bool(
            self.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_KEEPALIVE,
            keepalive,
        )
    }

    pub fn keepalive(&self) -> io::Result<bool> {
        get_int(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_KEEPALIVE).map(|value| value != 0)
    }

    pub fn set_reuseaddr(&self, reuseaddr: bool) -> io::Result<()> {
        set_bool(
            self.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            reuseaddr,
        )
    }

    pub fn reuseaddr(&self) -> io::Result<bool> {
        get_int(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_REUSEADDR).map(|value| value != 0)
    }

    pub fn set_reuseport(&self, reuseport: bool) -> io::Result<()> {
        set_bool(
            self.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_REUSEPORT,
            reuseport,
        )
    }

    pub fn reuseport(&self) -> io::Result<bool> {
        get_int(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_REUSEPORT).map(|value| value != 0)
    }

    pub fn set_send_buffer_size(&self, size: u32) -> io::Result<()> {
        set_int(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_SNDBUF, size)
    }

    pub fn send_buffer_size(&self) -> io::Result<u32> {
        get_int(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_SNDBUF).map(|value| value as u32)
    }

    pub fn set_recv_buffer_size(&self, size: u32) -> io::Result<()> {
        set_int(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVBUF, size)
    }

    pub fn recv_buffer_size(&self) -> io::Result<u32> {
        get_int(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVBUF).map(|value| value as u32)
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        set_bool(
            self.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_NODELAY,
            nodelay,
        )
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        get_int(self.as_raw_fd(), libc::IPPROTO_TCP, libc::TCP_NODELAY).map(|value| value != 0)
    }

    #[cfg(target_env = "ohos")]
    pub fn tclass_v6(&self) -> io::Result<u32> {
        u32::try_from(get_int(
            self.as_raw_fd(),
            libc::IPPROTO_IPV6,
            libc::IPV6_TCLASS,
        )?)
        .map_err(io::Error::other)
    }

    #[cfg(target_env = "ohos")]
    pub fn set_tclass_v6(&self, tclass: u32) -> io::Result<()> {
        set_int(
            self.as_raw_fd(),
            libc::IPPROTO_IPV6,
            libc::IPV6_TCLASS,
            tclass,
        )
    }

    #[cfg(target_env = "ohos")]
    pub fn tos_v4(&self) -> io::Result<u32> {
        u32::try_from(get_int(self.as_raw_fd(), libc::IPPROTO_IP, libc::IP_TOS)?)
            .map_err(io::Error::other)
    }

    #[cfg(target_env = "ohos")]
    pub fn tos(&self) -> io::Result<u32> {
        self.tos_v4()
    }

    #[cfg(target_env = "ohos")]
    pub fn set_tos_v4(&self, tos: u32) -> io::Result<()> {
        set_int(self.as_raw_fd(), libc::IPPROTO_IP, libc::IP_TOS, tos)
    }

    #[cfg(target_env = "ohos")]
    pub fn set_tos(&self, tos: u32) -> io::Result<()> {
        self.set_tos_v4(tos)
    }

    #[cfg(target_env = "ohos")]
    pub fn device(&self) -> io::Result<Option<Vec<u8>>> {
        socket_device(self.as_raw_fd())
    }

    #[cfg(target_env = "ohos")]
    pub fn bind_device(&self, interface: Option<&[u8]>) -> io::Result<()> {
        bind_socket_device(self.as_raw_fd(), interface)
    }

    pub fn set_linger(&self, duration: Option<Duration>) -> io::Result<()> {
        let linger = libc::linger {
            l_onoff: libc::c_int::from(duration.is_some()),
            l_linger: duration
                .map(|duration| duration.as_secs().min(i32::MAX as u64) as i32)
                .unwrap_or(0),
        };
        let result = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                (&linger as *const libc::linger).cast(),
                std::mem::size_of_val(&linger) as libc::socklen_t,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn set_zero_linger(&self) -> io::Result<()> {
        self.set_linger(Some(Duration::ZERO))
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        let mut linger = unsafe { std::mem::zeroed::<libc::linger>() };
        let mut length = std::mem::size_of_val(&linger) as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                self.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                (&mut linger as *mut libc::linger).cast(),
                &mut length,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((linger.l_onoff != 0).then(|| Duration::from_secs(linger.l_linger.max(0) as u64)))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let mut storage = unsafe { std::mem::zeroed::<libc::sockaddr_storage>() };
        let mut length = std::mem::size_of_val(&storage) as libc::socklen_t;
        let result = unsafe {
            libc::getsockname(
                self.as_raw_fd(),
                (&mut storage as *mut libc::sockaddr_storage).cast::<libc::sockaddr>(),
                &mut length,
            )
        };
        if result == 0 {
            decode_address(&storage)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        let error = get_int(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_ERROR)?;
        Ok((error != 0).then(|| io::Error::from_raw_os_error(error)))
    }

    pub fn bind(&self, address: SocketAddr) -> io::Result<()> {
        let (storage, length) = socket_address(address);
        let result = unsafe {
            libc::bind(
                self.as_raw_fd(),
                (&storage as *const libc::sockaddr_storage).cast::<libc::sockaddr>(),
                length,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub async fn connect(self, address: SocketAddr) -> io::Result<TcpStream> {
        let (storage, length) = socket_address(address);
        let result = unsafe {
            libc::connect(
                self.as_raw_fd(),
                (&storage as *const libc::sockaddr_storage).cast::<libc::sockaddr>(),
                length,
            )
        };
        let stream = unsafe { std::net::TcpStream::from_raw_fd(self.fd.into_raw_fd()) };
        if result == 0 {
            return TcpStream::from_std(stream);
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(libc::EINPROGRESS) | Some(libc::EALREADY)
        ) && error.kind() != io::ErrorKind::WouldBlock
        {
            return Err(error);
        }
        let stream = TcpStream::from_std(stream)?;
        stream.writable().await?;
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
        Ok(stream)
    }

    pub fn listen(self, backlog: u32) -> io::Result<TcpListener> {
        let result = unsafe { libc::listen(self.as_raw_fd(), backlog.min(i32::MAX as u32) as i32) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        let listener = unsafe { std::net::TcpListener::from_raw_fd(self.fd.into_raw_fd()) };
        TcpListener::from_std(listener)
    }

    pub fn from_std_stream(stream: std::net::TcpStream) -> Self {
        let _ = stream.set_nonblocking(true);
        Self {
            fd: unsafe { OwnedFd::from_raw_fd(stream.into_raw_fd()) },
        }
    }
}

impl AsRawFd for TcpSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl AsFd for TcpSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl std::fmt::Debug for TcpSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpSocket")
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}
