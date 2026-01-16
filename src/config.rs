// ============================================================================
// 配置管理模块
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 连接类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConnectionType {
    #[default]
    TcpClient,
    TcpServer,
    UdpOut,
    UdpIn,
    Udp,
    Serial,
}

impl ConnectionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionType::TcpClient => "TCP客户端",
            ConnectionType::TcpServer => "TCP服务端",
            ConnectionType::UdpOut => "UDP发送",
            ConnectionType::UdpIn => "UDP接收",
            ConnectionType::Udp => "UDP双向",
            ConnectionType::Serial => "串口",
        }
    }
    
    pub fn all() -> Vec<ConnectionType> {
        vec![
            ConnectionType::TcpClient,
            ConnectionType::TcpServer,
            ConnectionType::UdpOut,
            ConnectionType::UdpIn,
            ConnectionType::Udp,
            ConnectionType::Serial,
        ]
    }
    
    /// 是否为无连接协议（UDP）
    pub fn is_connectionless(&self) -> bool {
        matches!(self, ConnectionType::UdpOut | ConnectionType::UdpIn | ConnectionType::Udp)
    }
    
    /// 是否为TCP类型
    pub fn is_tcp(&self) -> bool {
        matches!(self, ConnectionType::TcpClient | ConnectionType::TcpServer)
    }
}

/// 连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub conn_type: ConnectionType,
    pub host: String,
    pub port: u16,
    pub local_port: u16,
    pub serial_port: String,
    pub baud_rate: u32,
    pub system_id: u8,
    pub component_id: u8,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            conn_type: ConnectionType::TcpClient,
            host: "127.0.0.1".to_string(),
            port: 5760,
            local_port: 14540,
            serial_port: String::new(),
            baud_rate: 57600,
            system_id: 255,
            component_id: 0,
        }
    }
}

impl ConnectionConfig {
    /// 生成连接地址字符串
    pub fn to_addr_string(&self) -> String {
        match self.conn_type {
            ConnectionType::TcpClient => format!("tcpout:{}:{}", self.host, self.port),
            ConnectionType::TcpServer => format!("tcpin:{}", self.port),
            ConnectionType::UdpOut => format!("udpout:{}:{}", self.host, self.port),
            ConnectionType::UdpIn => format!("udpin:0.0.0.0:{}", self.local_port),
            ConnectionType::Udp => format!("udp:{}:{}-{}", self.host, self.port, self.local_port),
            ConnectionType::Serial => format!("serial:{}:{}", self.serial_port, self.baud_rate),
        }
    }
}

/// 消息发送配置项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageConfig {
    pub id: String,                           // 唯一ID
    pub msg_name: String,                     // 消息名称
    pub msg_id: u32,                          // 消息ID
    pub enabled: bool,                        // 是否启用
    pub rate_hz: f32,                         // 发送频率 (Hz)
    pub fields: HashMap<String, FieldValue>,  // 字段值
    pub use_custom_header: bool,              // 使用自定义header
    pub header_system_id: u8,                 // 自定义 system_id
    pub header_component_id: u8,              // 自定义 component_id
}

/// 字段值类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    Number(f64),
    Text(String),
    Array(Vec<f64>),
}

impl Default for SendMessageConfig {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            msg_name: String::new(),
            msg_id: 0,
            enabled: false,
            rate_hz: 1.0,
            fields: HashMap::new(),
            use_custom_header: false,
            header_system_id: 255,
            header_component_id: 0,
        }
    }
}

/// 发送测试记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendTestRecord {
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub connection: ConnectionConfig,
    pub messages: Vec<SendMessageConfig>,
}

impl Default for SendTestRecord {
    fn default() -> Self {
        Self {
            name: "新建测试".to_string(),
            description: String::new(),
            created_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            connection: ConnectionConfig::default(),
            messages: Vec::new(),
        }
    }
}

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub window_id: u8,
    pub xml_path: String,
    pub last_record_name: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            window_id: 1,
            xml_path: "mavlink_xml/common.xml".to_string(),
            last_record_name: None,
        }
    }
}

/// 配置管理器
pub struct ConfigManager {
    config_dir: PathBuf,
    records_dir: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Self {
        let config_dir = Self::get_config_dir();
        let records_dir = config_dir.join("records");
        
        // 确保目录存在
        let _ = fs::create_dir_all(&config_dir);
        let _ = fs::create_dir_all(&records_dir);
        
        Self {
            config_dir,
            records_dir,
        }
    }
    
    fn get_config_dir() -> PathBuf {
        // 使用 exe 所在目录下的 .mav_testbed
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap())
            .join(".mav_testbed")
    }
    
    /// 加载应用配置
    pub fn load_app_config(&self) -> AppConfig {
        let path = self.config_dir.join("config.toml");
        if let Ok(content) = fs::read_to_string(&path) {
            toml::from_str(&content).unwrap_or_default()
        } else {
            AppConfig::default()
        }
    }
    
    /// 保存应用配置
    pub fn save_app_config(&self, config: &AppConfig) -> Result<(), String> {
        let path = self.config_dir.join("config.toml");
        let content = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
        fs::write(&path, content).map_err(|e| e.to_string())
    }
    
    /// 列出所有保存的测试记录
    pub fn list_records(&self) -> Vec<String> {
        let mut records = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.records_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".json") {
                        records.push(name.trim_end_matches(".json").to_string());
                    }
                }
            }
        }
        records.sort();
        records
    }
    
    /// 加载测试记录
    pub fn load_record(&self, name: &str) -> Result<SendTestRecord, String> {
        let path = self.records_dir.join(format!("{}.json", name));
        let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    }
    
    /// 保存测试记录
    pub fn save_record(&self, record: &SendTestRecord) -> Result<(), String> {
        let path = self.records_dir.join(format!("{}.json", record.name));
        let content = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
        fs::write(&path, content).map_err(|e| e.to_string())
    }
    
    /// 删除测试记录
    pub fn delete_record(&self, name: &str) -> Result<(), String> {
        let path = self.records_dir.join(format!("{}.json", name));
        fs::remove_file(&path).map_err(|e| e.to_string())
    }
    
    /// 获取配置目录路径
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }
    
    /// 获取记录目录路径
    pub fn records_dir(&self) -> &Path {
        &self.records_dir
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}
