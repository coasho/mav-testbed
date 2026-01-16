use mavlink::async_peek_reader::AsyncPeekReader;
use mavlink::{MavHeader, MavlinkVersion, Message, read_versioned_msg_async, write_versioned_msg_async};
use socket2::{Domain, Protocol, Socket, Type};
use std::marker::PhantomData;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf, split};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex as TokioMutex;
use tokio::time::{Duration, timeout};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

#[derive(Debug)]
pub enum MavError {
    Timeout,
    Io(String),
    Parse(String),
}

impl std::fmt::Display for MavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavError::Timeout => write!(f, "timeout"),
            MavError::Io(s) => write!(f, "io: {}", s),
            MavError::Parse(s) => write!(f, "parse: {}", s),
        }
    }
}

// ========== 串口 ==========

pub struct SerialSender<M: Message> {
    writer: WriteHalf<SerialStream>,
    version: MavlinkVersion,
    sequence: u8,
    system_id: u8,      // 新增
    component_id: u8,   // 新增
    _phantom: PhantomData<M>,
}

impl<M: Message> SerialSender<M> {
    pub async fn send(&mut self, msg: &M) -> Result<(), MavError> {
        let header = MavHeader { system_id: self.system_id, component_id: self.component_id, sequence: self.sequence };
        self.sequence = self.sequence.wrapping_add(1);
        self.send_with_header_internal(&header, msg).await
    }

    pub async fn send_with_header(&mut self, header: &MavHeader, msg: &M) -> Result<(), MavError> {
        let header =
            MavHeader { system_id: header.system_id, component_id: header.component_id, sequence: self.sequence };
        self.sequence = self.sequence.wrapping_add(1);
        self.send_with_header_internal(&header, msg).await
    }

    async fn send_with_header_internal(&mut self, header: &MavHeader, msg: &M) -> Result<(), MavError> {
        write_versioned_msg_async(&mut self.writer, self.version, *header, msg)
            .await
            .map_err(|e| MavError::Io(e.to_string()))?;
        self.writer.flush().await.map_err(|e| MavError::Io(e.to_string()))
    }

    /// 设置本机 system_id 和 component_id
    pub fn set_self_id(&mut self, sys: u8, comp: u8) {
        self.system_id = sys;
        self.component_id = comp;
    }
}

pub struct SerialReceiver<M: Message> {
    reader: AsyncPeekReader<ReadHalf<SerialStream>>,
    version: MavlinkVersion,
    _phantom: PhantomData<M>,
}

impl<M: Message> SerialReceiver<M> {
    pub async fn recv(&mut self) -> Result<(MavHeader, M), MavError> {
        read_versioned_msg_async::<M, _>(&mut self.reader, mavlink::ReadVersion::Single(self.version))
            .await
            .map_err(|e| MavError::Parse(e.to_string()))
    }

    pub async fn recv_timeout(&mut self, ms: u64) -> Result<(MavHeader, M), MavError> {
        timeout(Duration::from_millis(ms), self.recv()).await.unwrap_or(Err(MavError::Timeout))
    }
}

/// 根据IP地址查找对应的网络接口设备名
#[allow(dead_code)]
fn find_device_by_ip(ip: &str) -> Result<String, MavError> {
    let target_ip: IpAddr = ip.parse().map_err(|_| MavError::Io(format!("invalid IP address: {}", ip)))?;

    let interfaces = if_addrs::get_if_addrs().map_err(|e| MavError::Io(format!("failed to get interfaces: {}", e)))?;

    for iface in interfaces {
        if iface.ip() == target_ip {
            return Ok(iface.name);
        }
    }

    Err(MavError::Io(format!("no interface found for IP: {}", ip)))
}

pub fn open_serial<M: Message>(port: &str, baud: u32) -> Result<(SerialSender<M>, SerialReceiver<M>), MavError> {
    let serial = tokio_serial::new(port, baud).open_native_async().map_err(|e| MavError::Io(e.to_string()))?;
    let (rd, wr) = split(serial);
    Ok((
        SerialSender { writer: wr, version: MavlinkVersion::V2, sequence: 0, system_id: 255, component_id: 0, _phantom: PhantomData },
        SerialReceiver { reader: AsyncPeekReader::new(rd), version: MavlinkVersion::V2, _phantom: PhantomData },
    ))
}

// ========== TCP ==========

pub struct TcpSender<M: Message> {
    writer: Arc<TokioMutex<WriteHalf<TcpStream>>>,
    version: MavlinkVersion,
    sequence: Arc<AtomicU8>,
    system_id: Arc<AtomicU8>,      // 新增
    component_id: Arc<AtomicU8>,   // 新增
    _phantom: PhantomData<M>,
}

impl<M: Message> TcpSender<M> {
    pub async fn send(&self, msg: &M) -> Result<(), MavError> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let sys = self.system_id.load(Ordering::Relaxed);
        let comp = self.component_id.load(Ordering::Relaxed);
        let header = MavHeader { system_id: sys, component_id: comp, sequence: seq };
        self.send_with_header_internal(&header, msg).await
    }

    pub async fn send_with_header(&self, header: &MavHeader, msg: &M) -> Result<(), MavError> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let header = MavHeader { system_id: header.system_id, component_id: header.component_id, sequence: seq };
        self.send_with_header_internal(&header, msg).await
    }

    async fn send_with_header_internal(&self, header: &MavHeader, msg: &M) -> Result<(), MavError> {
        let mut writer = self.writer.lock().await;
        write_versioned_msg_async(&mut *writer, self.version, *header, msg)
            .await
            .map_err(|e| MavError::Io(e.to_string()))?;
        writer.flush().await.map_err(|e| MavError::Io(e.to_string()))
    }

    /// 设置本机 system_id 和 component_id
    pub fn set_self_id(&self, sys: u8, comp: u8) {
        self.system_id.store(sys, Ordering::Relaxed);
        self.component_id.store(comp, Ordering::Relaxed);
    }
}

impl<M: Message> Clone for TcpSender<M> {
    fn clone(&self) -> Self {
        Self {
            writer: self.writer.clone(),
            version: self.version,
            sequence: self.sequence.clone(),
            system_id: self.system_id.clone(),
            component_id: self.component_id.clone(),
            _phantom: PhantomData,
        }
    }
}

pub struct TcpReceiver<M: Message> {
    reader: Arc<TokioMutex<AsyncPeekReader<ReadHalf<TcpStream>>>>,
    version: MavlinkVersion,
    _phantom: PhantomData<M>,
}

impl<M: Message> TcpReceiver<M> {
    pub async fn recv(&mut self) -> Result<(MavHeader, M), MavError> {
        let mut reader = self.reader.lock().await;
        read_versioned_msg_async::<M, _>(&mut *reader, mavlink::ReadVersion::Single(self.version))
            .await
            .map_err(|e| MavError::Parse(e.to_string()))
    }

    pub async fn recv_timeout(&mut self, ms: u64) -> Result<(MavHeader, M), MavError> {
        timeout(Duration::from_millis(ms), self.recv()).await.unwrap_or(Err(MavError::Timeout))
    }
}

/// TCP 客户端连接
pub async fn open_tcp_client<M: Message>(addr: &str) -> Result<(TcpSender<M>, TcpReceiver<M>), MavError> {
    let stream = TcpStream::connect(addr).await.map_err(|e| MavError::Io(e.to_string()))?;
    let (rd, wr) = split(stream);
    Ok((
        TcpSender {
            writer: Arc::new(TokioMutex::new(wr)),
            version: MavlinkVersion::V2,
            sequence: Arc::new(AtomicU8::new(0)),
            system_id: Arc::new(AtomicU8::new(255)),
            component_id: Arc::new(AtomicU8::new(0)),
            _phantom: PhantomData,
        },
        TcpReceiver {
            reader: Arc::new(TokioMutex::new(AsyncPeekReader::new(rd))),
            version: MavlinkVersion::V2,
            _phantom: PhantomData,
        },
    ))
}

/// TCP 服务端监听，等待一个客户端连接
pub async fn open_tcp_server<M: Message>(port: u16) -> Result<(TcpSender<M>, TcpReceiver<M>), MavError> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await.map_err(|e| MavError::Io(e.to_string()))?;

    println!("[TCP] 监听端口 {}，等待连接...", port);

    let (stream, peer_addr) = listener.accept().await.map_err(|e| MavError::Io(e.to_string()))?;

    println!("[TCP] 客户端已连接: {}", peer_addr);

    let (rd, wr) = split(stream);
    Ok((
        TcpSender {
            writer: Arc::new(TokioMutex::new(wr)),
            version: MavlinkVersion::V2,
            sequence: Arc::new(AtomicU8::new(0)),
            system_id: Arc::new(AtomicU8::new(255)),
            component_id: Arc::new(AtomicU8::new(0)),
            _phantom: PhantomData,
        },
        TcpReceiver {
            reader: Arc::new(TokioMutex::new(AsyncPeekReader::new(rd))),
            version: MavlinkVersion::V2,
            _phantom: PhantomData,
        },
    ))
}

/// TCP 服务端监听，带超时
pub async fn open_tcp_server_timeout<M: Message>(
    port: u16,
    timeout_ms: u64,
) -> Result<(TcpSender<M>, TcpReceiver<M>), MavError> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await.map_err(|e| MavError::Io(e.to_string()))?;

    println!("[TCP] 监听端口 {}，等待连接 (超时 {}ms)...", port, timeout_ms);

    let accept_future = listener.accept();
    let (stream, peer_addr) = timeout(Duration::from_millis(timeout_ms), accept_future)
        .await
        .map_err(|_| MavError::Timeout)?
        .map_err(|e| MavError::Io(e.to_string()))?;

    println!("[TCP] 客户端已连接: {}", peer_addr);

    let (rd, wr) = split(stream);
    Ok((
        TcpSender {
            writer: Arc::new(TokioMutex::new(wr)),
            version: MavlinkVersion::V2,
            sequence: Arc::new(AtomicU8::new(0)),
            system_id: Arc::new(AtomicU8::new(255)),
            component_id: Arc::new(AtomicU8::new(0)),
            _phantom: PhantomData,
        },
        TcpReceiver {
            reader: Arc::new(TokioMutex::new(AsyncPeekReader::new(rd))),
            version: MavlinkVersion::V2,
            _phantom: PhantomData,
        },
    ))
}

// ========== UDP ==========

pub struct UdpSender<M: Message> {
    socket: Arc<UdpSocket>,
    version: MavlinkVersion,
    sequence: Arc<AtomicU8>,
    system_id: Arc<AtomicU8>,      // 新增
    component_id: Arc<AtomicU8>,   // 新增
    target_addr: Option<String>,
    _phantom: PhantomData<M>,
}

impl<M: Message> UdpSender<M> {
    pub async fn send(&self, msg: &M) -> Result<(), MavError> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let sys = self.system_id.load(Ordering::Relaxed);
        let comp = self.component_id.load(Ordering::Relaxed);
        let header = MavHeader { system_id: sys, component_id: comp, sequence: seq };
        self.send_with_header_internal(&header, msg).await
    }

    pub async fn send_with_header(&self, header: &MavHeader, msg: &M) -> Result<(), MavError> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let header = MavHeader { system_id: header.system_id, component_id: header.component_id, sequence: seq };
        self.send_with_header_internal(&header, msg).await
    }

    async fn send_with_header_internal(&self, header: &MavHeader, msg: &M) -> Result<(), MavError> {
        let mut buf = Vec::new();
        write_versioned_msg_async(&mut buf, self.version, *header, msg)
            .await
            .map_err(|e| MavError::Io(e.to_string()))?;

        if let Some(addr) = &self.target_addr {
            self.socket.send_to(&buf, addr).await.map_err(|e| MavError::Io(e.to_string()))?;
        } else {
            self.socket.send(&buf).await.map_err(|e| MavError::Io(e.to_string()))?;
        }
        Ok(())
    }

    /// 设置本机 system_id 和 component_id
    pub fn set_self_id(&self, sys: u8, comp: u8) {
        self.system_id.store(sys, Ordering::Relaxed);
        self.component_id.store(comp, Ordering::Relaxed);
    }
}

impl<M: Message> Clone for UdpSender<M> {
    fn clone(&self) -> Self {
        Self {
            socket: self.socket.clone(),
            version: self.version,
            sequence: self.sequence.clone(),
            system_id: self.system_id.clone(),
            component_id: self.component_id.clone(),
            target_addr: self.target_addr.clone(),
            _phantom: PhantomData,
        }
    }
}

pub struct UdpReceiver<M: Message> {
    socket: Arc<UdpSocket>,
    version: MavlinkVersion,
    _phantom: PhantomData<M>,
}

impl<M: Message> UdpReceiver<M> {
    pub async fn recv(&mut self) -> Result<(MavHeader, M), MavError> {
        let mut buf = [0u8; 65535];
        let n = self.socket.recv(&mut buf).await.map_err(|e| MavError::Io(e.to_string()))?;
        let mut cursor = std::io::Cursor::new(&buf[..n]);
        let mut reader = AsyncPeekReader::new(&mut cursor);
        read_versioned_msg_async::<M, _>(&mut reader, mavlink::ReadVersion::Single(self.version))
            .await
            .map_err(|e| MavError::Parse(e.to_string()))
    }

    pub async fn recv_timeout(&mut self, ms: u64) -> Result<(MavHeader, M), MavError> {
        timeout(Duration::from_millis(ms), self.recv()).await.unwrap_or(Err(MavError::Timeout))
    }
}

pub async fn open_udp_out<M: Message>(addr: &str) -> Result<(UdpSender<M>, UdpReceiver<M>), MavError> {
    let socket = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| MavError::Io(e.to_string()))?;
    socket.connect(addr).await.map_err(|e| MavError::Io(e.to_string()))?;
    let socket = Arc::new(socket);
    Ok((
        UdpSender {
            socket: socket.clone(),
            version: MavlinkVersion::V2,
            sequence: Arc::new(AtomicU8::new(0)),
            system_id: Arc::new(AtomicU8::new(255)),
            component_id: Arc::new(AtomicU8::new(0)),
            target_addr: None,
            _phantom: PhantomData,
        },
        UdpReceiver { socket, version: MavlinkVersion::V2, _phantom: PhantomData },
    ))
}

pub async fn open_udp_in<M: Message>(addr: &str) -> Result<(UdpSender<M>, UdpReceiver<M>), MavError> {
    let socket = UdpSocket::bind(addr).await.map_err(|e| MavError::Io(e.to_string()))?;
    let socket = Arc::new(socket);
    Ok((
        UdpSender {
            socket: socket.clone(),
            version: MavlinkVersion::V2,
            sequence: Arc::new(AtomicU8::new(0)),
            system_id: Arc::new(AtomicU8::new(255)),
            component_id: Arc::new(AtomicU8::new(0)),
            target_addr: None,
            _phantom: PhantomData,
        },
        UdpReceiver { socket, version: MavlinkVersion::V2, _phantom: PhantomData },
    ))
}

/// 打开UDP连接，发送和接收共用同一个socket
/// listen_port: 本地绑定端口（发送和接收都用这个端口）
/// target_addr: 发送目标地址 (ip:port)
/// bind_interface: 可选，绑定到指定网络接口
pub async fn open_udp_split<M: Message>(
    listen_port: u16,
    target_addr: &str,
    bind_interface: Option<&str>,
) -> Result<(UdpSender<M>, UdpReceiver<M>), MavError> {
    let socket = if let Some(interface_ip) = bind_interface {
        let socket =
            Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| MavError::Io(e.to_string()))?;

        #[cfg(target_os = "linux")]
        {
            let device_name = find_device_by_ip(interface_ip)?;
            socket
                .bind_device(Some(device_name.as_bytes()))
                .map_err(|e| MavError::Io(format!("bind device {} failed: {}", device_name, e)))?;
        }

        let bind_addr = format!("{}:{}", interface_ip, listen_port);
        socket
            .bind(&bind_addr.parse::<std::net::SocketAddr>().map_err(|e| MavError::Io(e.to_string()))?.into())
            .map_err(|e| MavError::Io(e.to_string()))?;

        socket.set_nonblocking(true).map_err(|e| MavError::Io(e.to_string()))?;

        UdpSocket::from_std(socket.into()).map_err(|e| MavError::Io(e.to_string()))?
    } else {
        let bind_addr = format!("0.0.0.0:{}", listen_port);
        UdpSocket::bind(&bind_addr).await.map_err(|e| MavError::Io(e.to_string()))?
    };

    let socket = Arc::new(socket);

    Ok((
        UdpSender {
            socket: socket.clone(),
            version: MavlinkVersion::V2,
            sequence: Arc::new(AtomicU8::new(0)),
            system_id: Arc::new(AtomicU8::new(255)),
            component_id: Arc::new(AtomicU8::new(0)),
            target_addr: Some(target_addr.to_string()),
            _phantom: PhantomData,
        },
        UdpReceiver { socket, version: MavlinkVersion::V2, _phantom: PhantomData },
    ))
}

// ========== 统一接口 ==========

pub enum Sender<M: Message> {
    Serial(SerialSender<M>),
    Udp(UdpSender<M>),
    Tcp(TcpSender<M>),
}

pub enum Receiver<M: Message> {
    Serial(SerialReceiver<M>),
    Udp(UdpReceiver<M>),
    Tcp(TcpReceiver<M>),
}

impl<M: Message> Sender<M> {
    pub async fn send(&mut self, msg: &M) -> Result<(), MavError> {
        match self {
            Sender::Serial(s) => s.send(msg).await,
            Sender::Udp(s) => s.send(msg).await,
            Sender::Tcp(s) => s.send(msg).await,
        }
    }

    pub async fn send_with_header(&mut self, header: &MavHeader, msg: &M) -> Result<(), MavError> {
        match self {
            Sender::Serial(s) => s.send_with_header(header, msg).await,
            Sender::Udp(s) => s.send_with_header(header, msg).await,
            Sender::Tcp(s) => s.send_with_header(header, msg).await,
        }
    }

    /// 设置本机 system_id 和 component_id
    pub fn set_self_id(&mut self, sys: u8, comp: u8) {
        match self {
            Sender::Serial(s) => s.set_self_id(sys, comp),
            Sender::Udp(s) => s.set_self_id(sys, comp),
            Sender::Tcp(s) => s.set_self_id(sys, comp),
        }
    }
}

impl<M: Message> Receiver<M> {
    pub async fn recv(&mut self) -> Result<(MavHeader, M), MavError> {
        match self {
            Receiver::Serial(r) => r.recv().await,
            Receiver::Udp(r) => r.recv().await,
            Receiver::Tcp(r) => r.recv().await,
        }
    }

    pub async fn recv_timeout(&mut self, ms: u64) -> Result<(MavHeader, M), MavError> {
        match self {
            Receiver::Serial(r) => r.recv_timeout(ms).await,
            Receiver::Udp(r) => r.recv_timeout(ms).await,
            Receiver::Tcp(r) => r.recv_timeout(ms).await,
        }
    }
}

pub async fn open<M: Message>(addr: &str) -> Result<(Sender<M>, Receiver<M>), MavError> {
    let parts: Vec<&str> = addr.splitn(2, ':').collect();
    if parts.len() < 2 {
        return Err(MavError::Io("invalid address".into()));
    }
    match parts[0] {
        "serial" => {
            let p: Vec<&str> = parts[1].split(':').collect();
            if p.len() < 2 {
                return Err(MavError::Io("serial:PORT:BAUD".into()));
            }
            let (tx, rx) = open_serial(p[0], p[1].parse().map_err(|_| MavError::Io("bad baud".into()))?)?;
            Ok((Sender::Serial(tx), Receiver::Serial(rx)))
        }
        "udpout" => {
            let (tx, rx) = open_udp_out(parts[1]).await?;
            Ok((Sender::Udp(tx), Receiver::Udp(rx)))
        }
        "udpin" => {
            let (tx, rx) = open_udp_in(parts[1]).await?;
            Ok((Sender::Udp(tx), Receiver::Udp(rx)))
        }
        "udp" => {
            // 格式: udp:{ip}:{target_port}-{local_port}?if={interface}
            // 例如: udp:127.0.0.1:14580-14540 表示本地绑定14540，发送到127.0.0.1:14580
            let (addr_part, query_part) = if let Some(pos) = parts[1].find('?') {
                (&parts[1][..pos], Some(&parts[1][pos + 1..]))
            } else {
                (parts[1], None)
            };

            let bind_interface = if let Some(query) = query_part {
                let params: Vec<&str> = query.split('&').collect();
                let mut interface = None;
                for param in params {
                    if let Some(value) = param.strip_prefix("if=") {
                        interface = Some(value);
                    }
                }
                interface
            } else {
                None
            };

            let p: Vec<&str> = addr_part.split(':').collect();
            if p.len() < 2 {
                return Err(MavError::Io("udp:{ip}:{target_port}-{local_port}?if={interface}".into()));
            }
            let ip = p[0];
            let ports: Vec<&str> = p[1].split('-').collect();
            if ports.len() != 2 {
                return Err(MavError::Io("udp:{ip}:{target_port}-{local_port}?if={interface}".into()));
            }
            let target_port: u16 = ports[0].parse().map_err(|_| MavError::Io("bad target_port".into()))?;
            let local_port: u16 = ports[1].parse().map_err(|_| MavError::Io("bad local_port".into()))?;

            let target_addr = format!("{}:{}", ip, target_port);
            let (tx, rx) = open_udp_split(local_port, &target_addr, bind_interface).await?;
            Ok((Sender::Udp(tx), Receiver::Udp(rx)))
        }
        // TCP 客户端: tcpout:ip:port
        "tcpout" => {
            let (tx, rx) = open_tcp_client(parts[1]).await?;
            Ok((Sender::Tcp(tx), Receiver::Tcp(rx)))
        }
        // TCP 服务端: tcpin:port (监听并等待连接)
        "tcpin" => {
            let port: u16 = parts[1].parse().map_err(|_| MavError::Io("bad port".into()))?;
            let (tx, rx) = open_tcp_server(port).await?;
            Ok((Sender::Tcp(tx), Receiver::Tcp(rx)))
        }
        // TCP 服务端简写: tcp:port
        "tcp" => {
            let port: u16 = parts[1].parse().map_err(|_| MavError::Io("bad port".into()))?;
            let (tx, rx) = open_tcp_server(port).await?;
            Ok((Sender::Tcp(tx), Receiver::Tcp(rx)))
        }
        str => Err(MavError::Io(format!("unknown protocol:{}", str).into())),
    }
}