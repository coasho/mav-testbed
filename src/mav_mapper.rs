use mavlink::Message;
use mavlink::common::MavMessage;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// MAVLink 消息定义
#[derive(Debug, Clone)]
pub struct MessageDef {
    pub id: u32,
    pub name: String,
    pub fields: Vec<FieldDef>,
    pub crc_extra: u8,
}

/// 字段定义
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub field_type: String,
    pub name: String,
    pub units: String,
    pub enum_type: Option<String>, // 新增：枚举类型名称
    pub is_extension: bool,        // 新增：是否为扩展字段
}

/// 字段类型枚举
#[derive(Debug, Clone)]
pub enum FieldType {
    Float,
    Double,
    Int8,
    UInt8,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    CharArray(usize),
    Array(Box<FieldType>, usize),
}

impl FieldType {
    pub fn from_string(type_str: &str) -> Self {
        if let Some(start) = type_str.find('[') {
            if let Some(end) = type_str.find(']') {
                let base_type = &type_str[..start];
                let size: usize = type_str[start + 1..end].parse().unwrap_or(1);
                if base_type == "char" {
                    return FieldType::CharArray(size);
                } else {
                    return FieldType::Array(Box::new(Self::from_string(base_type)), size);
                }
            }
        }
        match type_str {
            "float" => FieldType::Float,
            "double" => FieldType::Double,
            "int8_t" => FieldType::Int8,
            "uint8_t" => FieldType::UInt8,
            "int16_t" => FieldType::Int16,
            "uint16_t" => FieldType::UInt16,
            "int32_t" => FieldType::Int32,
            "uint32_t" => FieldType::UInt32,
            "int64_t" => FieldType::Int64,
            "uint64_t" => FieldType::UInt64,
            _ => FieldType::Float,
        }
    }
    /// 获取用于排序的大小（数组按元素类型大小排序）
    pub fn sort_size(&self) -> usize {
        match self {
            FieldType::Array(base, _) => base.size(), // 使用元素大小
            FieldType::CharArray(_) => 1,             // char 大小为 1
            _ => self.size(),
        }
    }
    pub fn size(&self) -> usize {
        match self {
            FieldType::Int8 | FieldType::UInt8 => 1,
            FieldType::Int16 | FieldType::UInt16 => 2,
            FieldType::Int32 | FieldType::UInt32 | FieldType::Float => 4,
            FieldType::Int64 | FieldType::UInt64 | FieldType::Double => 8,
            FieldType::CharArray(n) => *n,
            FieldType::Array(base, n) => base.size() * n,
        }
    }

    pub fn read_value(&self, bytes: &[u8], offset: usize) -> Option<f64> {
        if offset + self.size() > bytes.len() {
            return None;
        }
        let slice = &bytes[offset..offset + self.size()];
        Some(match self {
            FieldType::Float => f32::from_le_bytes(slice.try_into().ok()?) as f64,
            FieldType::Double => f64::from_le_bytes(slice.try_into().ok()?),
            FieldType::Int8 => i8::from_le_bytes([slice[0]]) as f64,
            FieldType::UInt8 => slice[0] as f64,
            FieldType::Int16 => i16::from_le_bytes(slice.try_into().ok()?) as f64,
            FieldType::UInt16 => u16::from_le_bytes(slice.try_into().ok()?) as f64,
            FieldType::Int32 => i32::from_le_bytes(slice.try_into().ok()?) as f64,
            FieldType::UInt32 => u32::from_le_bytes(slice.try_into().ok()?) as f64,
            FieldType::Int64 => i64::from_le_bytes(slice.try_into().ok()?) as f64,
            FieldType::UInt64 => u64::from_le_bytes(slice.try_into().ok()?) as f64,
            _ => 0.0,
        })
    }

    pub fn read_array_element(
        &self,
        bytes: &[u8],
        base_offset: usize,
        index: usize,
    ) -> Option<f64> {
        match self {
            FieldType::Array(elem_type, _) => {
                let elem_offset = base_offset + elem_type.size() * index;
                elem_type.read_value(bytes, elem_offset)
            }
            FieldType::CharArray(_) => {
                // char数组：每个元素1字节
                let elem_offset = base_offset + index;
                if elem_offset < bytes.len() {
                    Some(bytes[elem_offset] as f64)
                } else {
                    None
                }
            }
            _ => self.read_value(bytes, base_offset),
        }
    }

    pub fn write_value(&self, bytes: &mut [u8], offset: usize, value: f64) -> Option<()> {
        if offset + self.size() > bytes.len() {
            return None;
        }
        let slice = &mut bytes[offset..offset + self.size()];
        match self {
            FieldType::Float => slice.copy_from_slice(&(value as f32).to_le_bytes()),
            FieldType::Double => slice.copy_from_slice(&value.to_le_bytes()),
            FieldType::Int8 => slice[0] = value as i8 as u8,
            FieldType::UInt8 => slice[0] = value as u8,
            FieldType::Int16 => slice.copy_from_slice(&(value as i16).to_le_bytes()),
            FieldType::UInt16 => slice.copy_from_slice(&(value as u16).to_le_bytes()),
            FieldType::Int32 => slice.copy_from_slice(&(value as i32).to_le_bytes()),
            FieldType::UInt32 => slice.copy_from_slice(&(value as u32).to_le_bytes()),
            FieldType::Int64 => slice.copy_from_slice(&(value as i64).to_le_bytes()),
            FieldType::UInt64 => slice.copy_from_slice(&(value as u64).to_le_bytes()),
            _ => {}
        }
        Some(())
    }

    pub fn write_array_element(
        &self,
        bytes: &mut [u8],
        base_offset: usize,
        index: usize,
        value: f64,
    ) -> Option<()> {
        match self {
            FieldType::Array(elem_type, _) => {
                let elem_offset = base_offset + elem_type.size() * index;
                elem_type.write_value(bytes, elem_offset, value)
            }
            FieldType::CharArray(_) => {
                // char 数组，每个元素是 1 字节
                let elem_offset = base_offset + index;
                if elem_offset < bytes.len() {
                    bytes[elem_offset] = value as u8;
                    Some(())
                } else {
                    None
                }
            }
            _ => self.write_value(bytes, base_offset, value),
        }
    }

    pub fn array_length(&self) -> usize {
        match self {
            FieldType::Array(_, len) => *len,
            FieldType::CharArray(len) => *len,
            _ => 1,
        }
    }

    pub fn is_array(&self) -> bool {
        matches!(self, FieldType::Array(_, _) | FieldType::CharArray(_))
    }

    /// 判断是否为char数组类型
    pub fn is_char_array(&self) -> bool {
        matches!(self, FieldType::CharArray(_))
    }
}

/// MAVLink 枚举定义
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub entries: Vec<EnumEntry>,
    pub bitmask: bool, // 是否为位掩码类型
}

/// 枚举项定义
#[derive(Debug, Clone)]
pub struct EnumEntry {
    pub name: String,
    pub value: u64,
    pub description: String,
}

/// MavMapper 类 - MAVLink消息映射器
pub struct MavMapper {
    /// 消息元数据 (msg_id -> MessageDef)
    message_metadata: HashMap<u32, MessageDef>,
    /// 消息名到ID的映射
    name_to_id: HashMap<String, u32>,
    /// 枚举定义 (enum_name -> EnumDef)
    enum_definitions: HashMap<String, EnumDef>,
    /// 系统ID
    system_id: u8,
    /// 组件ID
    component_id: u8,
}

impl MavMapper {
    /// 创建新的 MavMapper 实例
    pub fn new<P: AsRef<Path>>(xml_path: P) -> Result<Self, String> {
        let mut mapper = MavMapper {
            message_metadata: HashMap::new(),
            name_to_id: HashMap::new(),
            enum_definitions: HashMap::new(),
            system_id: 1,
            component_id: 1,
        };

        mapper.load_xml(xml_path)?;
        Ok(mapper)
    }

    /// 设置系统ID
    pub fn set_system_id(&mut self, system_id: u8) {
        self.system_id = system_id;
    }

    /// 设置组件ID
    pub fn set_component_id(&mut self, component_id: u8) {
        self.component_id = component_id;
    }

    /// 获取枚举定义
    pub fn get_enum_def(&self, enum_name: &str) -> Option<&EnumDef> {
        self.enum_definitions.get(enum_name)
    }

    /// 获取所有枚举名称
    pub fn get_all_enum_names(&self) -> Vec<&str> {
        self.enum_definitions.keys().map(|s| s.as_str()).collect()
    }

    /// 根据枚举名和值获取枚举项名称
    pub fn get_enum_entry_name(&self, enum_name: &str, value: u64) -> Option<&str> {
        self.enum_definitions.get(enum_name).and_then(|e| {
            e.entries
                .iter()
                .find(|entry| entry.value == value)
                .map(|entry| entry.name.as_str())
        })
    }

    /// 根据枚举名和项名获取值
    pub fn get_enum_entry_value(&self, enum_name: &str, entry_name: &str) -> Option<u64> {
        self.enum_definitions.get(enum_name).and_then(|e| {
            e.entries
                .iter()
                .find(|entry| entry.name == entry_name)
                .map(|entry| entry.value)
        })
    }

    /// 加载 MAVLink XML 定义文件
    fn load_xml<P: AsRef<Path>>(&mut self, xml_path: P) -> Result<(), String> {
        let path = xml_path.as_ref();
        let mut parsed_files = std::collections::HashSet::new();
        let (messages, enums) = self.parse_mavlink_xml_recursive(path, &mut parsed_files)?;

        for msg_def in messages {
            self.name_to_id.insert(msg_def.name.clone(), msg_def.id);
            self.message_metadata.entry(msg_def.id).or_insert(msg_def);
        }

        for enum_def in enums {
            self.enum_definitions
                .entry(enum_def.name.clone())
                .or_insert(enum_def);
        }

        println!(
            "✓ 总共加载 {} 个消息定义, {} 个枚举定义",
            self.message_metadata.len(),
            self.enum_definitions.len()
        );
        Ok(())
    }

    /// 递归解析 MAVLink XML（支持 include）
    fn parse_mavlink_xml_recursive(
        &self,
        xml_path: &Path,
        parsed_files: &mut std::collections::HashSet<std::path::PathBuf>,
    ) -> Result<(Vec<MessageDef>, Vec<EnumDef>), String> {
        let canonical_path = xml_path
            .canonicalize()
            .map_err(|e| format!("无法解析路径 {:?}: {:?}", xml_path, e))?;

        if parsed_files.contains(&canonical_path) {
            return Ok((Vec::new(), Vec::new()));
        }
        parsed_files.insert(canonical_path.clone());

        println!("  解析: {:?}", xml_path);

        let xml_str = fs::read_to_string(xml_path)
            .map_err(|e| format!("读取失败 {:?}: {:?}", xml_path, e))?;

        let mut all_messages = Vec::new();
        let mut all_enums = Vec::new();
        let mut reader = Reader::from_str(&xml_str);
        reader.config_mut().trim_text(true);

        let mut current_message: Option<MessageDef> = None;
        let mut current_field: Option<FieldDef> = None;
        let mut current_enum: Option<EnumDef> = None;
        let mut current_entry: Option<EnumEntry> = None;
        let mut in_messages = false;
        let mut in_enums = false;
        let mut in_extensions = false;
        let mut buf = Vec::new();
        let mut text_buf = String::new();

        let base_dir = xml_path.parent().ok_or("无法获取文件目录")?;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => match e.name().as_ref() {
                    b"include" => {
                        text_buf.clear();
                    }
                    b"enums" => in_enums = true,
                    b"messages" => in_messages = true,
                    b"extensions" => in_extensions = true,

                    b"enum" if in_enums => {
                        let mut enum_def = EnumDef {
                            name: String::new(),
                            entries: Vec::new(),
                            bitmask: false,
                        };
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"name" => {
                                    enum_def.name =
                                        String::from_utf8_lossy(&attr.value).to_string();
                                }
                                b"bitmask" => {
                                    let val = String::from_utf8_lossy(&attr.value);
                                    enum_def.bitmask = val == "true" || val == "1";
                                }
                                _ => {}
                            }
                        }
                        current_enum = Some(enum_def);
                    }

                    b"entry" if current_enum.is_some() => {
                        let mut entry = EnumEntry {
                            name: String::new(),
                            value: 0,
                            description: String::new(),
                        };
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"name" => {
                                    entry.name = String::from_utf8_lossy(&attr.value).to_string();
                                }
                                b"value" => {
                                    let val_str = String::from_utf8_lossy(&attr.value);
                                    entry.value =
                                        if val_str.starts_with("0x") || val_str.starts_with("0X") {
                                            u64::from_str_radix(&val_str[2..], 16).unwrap_or(0)
                                        } else {
                                            val_str.parse().unwrap_or(0)
                                        };
                                }
                                _ => {}
                            }
                        }
                        current_entry = Some(entry);
                    }

                    b"message" if in_messages => {
                        let mut msg = MessageDef {
                            id: 0,
                            name: String::new(),
                            fields: Vec::new(),
                            crc_extra: 0,
                        };
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"id" => {
                                    msg.id = Self::parse_attr(&attr.value, 0);
                                }
                                b"name" => {
                                    msg.name = String::from_utf8_lossy(&attr.value).to_string();
                                }
                                _ => {}
                            }
                        }
                        in_extensions = false;
                        current_message = Some(msg);
                    }

                    b"field" if current_message.is_some() => {
                        let mut field = FieldDef {
                            field_type: String::new(),
                            name: String::new(),
                            units: String::new(),
                            enum_type: None,
                            is_extension: in_extensions,
                        };
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"type" => {
                                    field.field_type =
                                        String::from_utf8_lossy(&attr.value).to_string();
                                }
                                b"name" => {
                                    field.name = String::from_utf8_lossy(&attr.value).to_string();
                                }
                                b"units" => {
                                    field.units = String::from_utf8_lossy(&attr.value).to_string();
                                }
                                b"enum" => {
                                    field.enum_type =
                                        Some(String::from_utf8_lossy(&attr.value).to_string());
                                }
                                _ => {}
                            }
                        }
                        current_field = Some(field);
                    }
                    _ => {}
                },

                Ok(Event::Empty(e)) => {
                    // 处理自闭合标签如 <extensions/>
                    if e.name().as_ref() == b"extensions" {
                        in_extensions = true;
                    }
                }

                Ok(Event::Text(e)) => {
                    text_buf = String::from_utf8_lossy(&e).to_string();
                }

                Ok(Event::End(e)) => match e.name().as_ref() {
                    b"include" => {
                        let include_file = text_buf.trim();
                        if !include_file.is_empty() {
                            let include_path = base_dir.join(include_file);
                            match self.parse_mavlink_xml_recursive(&include_path, parsed_files) {
                                Ok((mut included_messages, mut included_enums)) => {
                                    println!(
                                        "    ✓ 从 {} 加载 {} 个消息, {} 个枚举",
                                        include_file,
                                        included_messages.len(),
                                        included_enums.len()
                                    );
                                    all_messages.append(&mut included_messages);
                                    all_enums.append(&mut included_enums);
                                }
                                Err(e) => {
                                    eprintln!("    ✗ 解析include文件失败 {}: {}", include_file, e);
                                }
                            }
                        }
                        text_buf.clear();
                    }
                    b"enums" => in_enums = false,
                    b"messages" => in_messages = false,

                    b"entry" => {
                        if let Some(mut entry) = current_entry.take() {
                            if !text_buf.trim().is_empty() {
                                entry.description = text_buf.trim().to_string();
                            }
                            if let Some(enum_def) = current_enum.as_mut() {
                                if !entry.name.is_empty() {
                                    enum_def.entries.push(entry);
                                }
                            }
                        }
                        text_buf.clear();
                    }

                    b"enum" => {
                        if let Some(enum_def) = current_enum.take() {
                            if !enum_def.name.is_empty() {
                                all_enums.push(enum_def);
                            }
                        }
                    }

                    b"message" => {
                        in_extensions = false;
                        if let Some(mut msg) = current_message.take() {
                            if !msg.name.is_empty() {
                                msg.crc_extra = Self::calculate_crc_extra(&msg);
                                all_messages.push(msg);
                            }
                        }
                    }

                    b"field" => {
                        if let Some(field) = current_field.take() {
                            if let Some(msg) = current_message.as_mut() {
                                if !field.field_type.is_empty() && !field.name.is_empty() {
                                    msg.fields.push(field);
                                }
                            }
                        }
                    }
                    _ => {}
                },

                Ok(Event::Eof) => break,
                Err(e) => return Err(format!("XML错误 {:?}: {:?}", xml_path, e)),
                _ => {}
            }
            buf.clear();
        }

        Ok((all_messages, all_enums))
    }

    fn parse_attr<T: std::str::FromStr>(value: &[u8], default: T) -> T {
        std::str::from_utf8(value)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    }

    /// 计算 CRC_EXTRA (注意：扩展字段不参与 CRC 计算)
    fn calculate_crc_extra(msg_def: &MessageDef) -> u8 {
        let mut crc: u16 = 0xFFFF;

        for &b in format!("{} ", msg_def.name).as_bytes() {
            let tmp = (b as u16) ^ (crc & 0xFF);
            let tmp = (tmp ^ (tmp << 4)) & 0xFF;
            crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
        }

        // 只处理非扩展字段
        let mut fields_with_type: Vec<_> = msg_def
            .fields
            .iter()
            .filter(|f| !f.is_extension) // 过滤掉扩展字段
            .map(|f| (f, FieldType::from_string(&f.field_type)))
            .collect();

        fields_with_type.sort_by(|a, b| b.1.size().cmp(&a.1.size()));

        for (field, field_type) in fields_with_type {
            for &b in format!("{} ", field.field_type).as_bytes() {
                let tmp = (b as u16) ^ (crc & 0xFF);
                let tmp = (tmp ^ (tmp << 4)) & 0xFF;
                crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
            }

            for &b in format!("{} ", field.name).as_bytes() {
                let tmp = (b as u16) ^ (crc & 0xFF);
                let tmp = (tmp ^ (tmp << 4)) & 0xFF;
                crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
            }

            if let FieldType::Array(_, len) = field_type {
                let b = len as u8;
                let tmp = (b as u16) ^ (crc & 0xFF);
                let tmp = (tmp ^ (tmp << 4)) & 0xFF;
                crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
            } else if let FieldType::CharArray(len) = field_type {
                let b = len as u8;
                let tmp = (b as u16) ^ (crc & 0xFF);
                let tmp = (tmp ^ (tmp << 4)) & 0xFF;
                crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
            }
        }

        ((crc & 0xFF) ^ (crc >> 8)) as u8
    }

    /// 获取消息的字段排序信息（按MAVLink规范排序）
    /// 注意：扩展字段保持原始顺序，放在非扩展字段之后
    fn get_sorted_field_info(
        &self,
        msg_def: &MessageDef,
    ) -> Vec<(String, FieldType, String, usize)> {
        // 分离非扩展字段和扩展字段
        let (non_ext_fields, ext_fields): (Vec<_>, Vec<_>) = msg_def
            .fields
            .iter()
            .map(|f| (f, FieldType::from_string(&f.field_type)))
            .partition(|(f, _)| !f.is_extension);

        // 非扩展字段按**元素类型大小**降序排序
        let mut sorted_non_ext: Vec<_> = non_ext_fields;
        sorted_non_ext.sort_by(|a, b| b.1.sort_size().cmp(&a.1.sort_size())); // ← 使用 sort_size()

        let mut offset = 0;
        let mut field_info = Vec::new();

        // 先处理非扩展字段
        for (field, field_type) in sorted_non_ext {
            field_info.push((
                field.name.clone(),
                field_type.clone(),
                field.units.clone(),
                offset,
            ));
            offset += field_type.size();
        }

        // 再处理扩展字段（保持原始顺序）
        for (field, field_type) in ext_fields {
            field_info.push((
                field.name.clone(),
                field_type.clone(),
                field.units.clone(),
                offset,
            ));
            offset += field_type.size();
        }

        field_info
    }

    /// 获取消息的字段排序信息（包含枚举信息）
    pub fn get_sorted_field_info_with_enum(
        &self,
        msg_def: &MessageDef,
    ) -> Vec<(String, FieldType, String, Option<String>, bool, usize)> {
        // 分离非扩展字段和扩展字段
        let (non_ext_fields, ext_fields): (Vec<_>, Vec<_>) = msg_def
            .fields
            .iter()
            .map(|f| (f, FieldType::from_string(&f.field_type)))
            .partition(|(f, _)| !f.is_extension);

        // 非扩展字段按大小降序排序
        let mut sorted_non_ext: Vec<_> = non_ext_fields;
        sorted_non_ext.sort_by(|a, b| b.1.size().cmp(&a.1.size()));

        let mut offset = 0;
        let mut field_info = Vec::new();

        // 先处理非扩展字段
        for (field, field_type) in sorted_non_ext {
            field_info.push((
                field.name.clone(),
                field_type.clone(),
                field.units.clone(),
                field.enum_type.clone(),
                field.is_extension,
                offset,
            ));
            offset += field_type.size();
        }

        // 再处理扩展字段（保持原始顺序）
        for (field, field_type) in ext_fields {
            field_info.push((
                field.name.clone(),
                field_type.clone(),
                field.units.clone(),
                field.enum_type.clone(),
                field.is_extension,
                offset,
            ));
            offset += field_type.size();
        }

        field_info
    }

    /// 根据消息ID获取 MavMessage
    pub fn get_mavlink_msg(&self, msg_id: u32, metas: &HashMap<String, f64>) -> Option<MavMessage> {
        let msg_def = self.message_metadata.get(&msg_id)?;
        let msg_name = msg_def.name.clone();

        let field_info = self.get_sorted_field_info(msg_def);
        let payload_size: usize = field_info.iter().map(|(_, ft, _, _)| ft.size()).sum();
        let mut payload = vec![0u8; payload_size];

        let mut found_any_value = false;

        for (field_name, field_type, _units, offset) in &field_info {
            let array_len = field_type.array_length();
            if field_type.is_array() && array_len > 1 {
                for i in 0..array_len {
                    let indexed_name = format!("{}[{}]", field_name, i);
                    let key = format!("{}:{}", msg_name, indexed_name);
                    if let Some(&value) = metas.get(&key) {
                        found_any_value = true;
                        field_type.write_array_element(&mut payload, *offset, i, value);
                    }
                }
            } else {
                let key = format!("{}:{}", msg_name, field_name);
                if let Some(&value) = metas.get(&key) {
                    found_any_value = true;
                    field_type.write_value(&mut payload, *offset, value);
                }
            }
        }

        if !found_any_value {
            return None;
        }

        MavMessage::parse(mavlink::MavlinkVersion::V2, msg_id, &payload).ok()
    }

    /// 根据消息ID获取 MavMessage，同时自动填充 target_system 和 target_component
    pub fn get_mavlink_msg_with_target(
        &self,
        msg_id: u32,
        metas: &HashMap<String, f64>,
        target_system: u8,
        target_component: u8,
    ) -> Option<MavMessage> {
        let msg_def = self.message_metadata.get(&msg_id)?;
        let msg_name = msg_def.name.clone();

        let field_info = self.get_sorted_field_info(msg_def);
        let payload_size: usize = field_info.iter().map(|(_, ft, _, _)| ft.size()).sum();
        let mut payload = vec![0u8; payload_size];

        let mut found_any_value = false;

        for (field_name, field_type, _units, offset) in &field_info {
            // 自动填充 target 字段
            let value = match field_name.as_str() {
                "target_system" => {
                    found_any_value = true;
                    Some(target_system as f64)
                }
                "target_component" => {
                    found_any_value = true;
                    Some(target_component as f64)
                }
                "target" => {
                    // MANUAL_CONTROL 等消息用 target 而不是 target_system
                    found_any_value = true;
                    Some(target_system as f64)
                }
                _ => {
                    // 从 metas 读取
                    let array_len = field_type.array_length();
                    if field_type.is_array() && array_len > 1 {
                        for i in 0..array_len {
                            let indexed_name = format!("{}[{}]", field_name, i);
                            let key = format!("{}:{}", msg_name, indexed_name);
                            if let Some(&v) = metas.get(&key) {
                                found_any_value = true;
                                field_type.write_array_element(&mut payload, *offset, i, v);
                            }
                        }
                        None // 数组已处理
                    } else {
                        let key = format!("{}:{}", msg_name, field_name);
                        metas.get(&key).copied()
                    }
                }
            };

            if let Some(v) = value {
                found_any_value = true;
                field_type.write_value(&mut payload, *offset, v);
            }
        }

        if !found_any_value {
            return None;
        }

        MavMessage::parse(mavlink::MavlinkVersion::V2, msg_id, &payload).ok()
    }

    /// 根据消息名获取 MavMessage
    pub fn get_mavlink_msg_by_name(
        &self,
        name: &str,
        metas: &HashMap<String, f64>,
    ) -> Option<MavMessage> {
        let msg_id = self.name_to_id.get(name)?;
        self.get_mavlink_msg(*msg_id, metas)
    }

    /// 根据消息名获取 MavMessage，同时自动填充 target_system 和 target_component
    pub fn get_mavlink_msg_by_name_with_target(
        &self,
        name: &str,
        metas: &HashMap<String, f64>,
        target_system: u8,
        target_component: u8,
    ) -> Option<MavMessage> {
        let msg_id = self.name_to_id.get(name)?;
        self.get_mavlink_msg_with_target(*msg_id, metas, target_system, target_component)
    }

    /// 解析 MavMessage 并通过回调输出字段值
    pub fn parsing_mavlink_msg(&self, msg: &MavMessage, metas: &mut HashMap<String, f64>) {
        let msg_id = msg.message_id();

        let mut buf = vec![0u8; 255];
        let len = msg.ser(mavlink::MavlinkVersion::V2, &mut buf);
        let payload = &buf[..len];

        self.parse_payload(msg_id, payload, metas);
    }

    /// 解析 MAVLink 消息字节数组
    pub fn parsing_mavlink_msg_bytes(
        &self,
        bytes: &[u8],
        metas: &mut HashMap<String, f64>,
    ) -> Option<u32> {
        if bytes.len() < 12 || bytes[0] != 0xFD {
            if bytes.len() >= 8 && bytes[0] == 0xFE {
                return self.parse_mavlink_v1_bytes(bytes, metas);
            }
            return None;
        }

        let payload_len = bytes[1] as usize;
        let msg_id = (bytes[7] as u32) | ((bytes[8] as u32) << 8) | ((bytes[9] as u32) << 16);

        let header_len = 10;
        let expected_len = header_len + payload_len + 2;

        if bytes.len() < expected_len {
            return None;
        }

        let payload = &bytes[header_len..header_len + payload_len];
        self.parse_payload(msg_id, payload, metas);
        Some(msg_id)
    }

    /// 解析 MAVLink v1 字节数组
    fn parse_mavlink_v1_bytes(
        &self,
        bytes: &[u8],
        metas: &mut HashMap<String, f64>,
    ) -> Option<u32> {
        if bytes.len() < 8 || bytes[0] != 0xFE {
            return None;
        }

        let payload_len = bytes[1] as usize;
        let msg_id = bytes[5] as u32;

        let header_len = 6;
        let expected_len = header_len + payload_len + 2;

        if bytes.len() < expected_len {
            return None;
        }

        let payload = &bytes[header_len..header_len + payload_len];
        self.parse_payload(msg_id, payload, metas);
        Some(msg_id)
    }

    /// 解析 payload 并写入 metas
    /// 注意：MAVLink v2 会截断尾部的零字节，所以超出payload长度的字段值默认为0
    fn parse_payload(&self, msg_id: u32, payload: &[u8], metas: &mut HashMap<String, f64>) {
        let msg_def = match self.message_metadata.get(&msg_id) {
            Some(def) => def,
            None => return,
        };

        let msg_name = &msg_def.name;
        let field_info = self.get_sorted_field_info(msg_def);

        for (field_name, field_type, _units, offset) in &field_info {
            let array_len = field_type.array_length();

            if field_type.is_char_array() {
                // char数组：读取字节并存储，同时存储一个字符串形式的值
                let mut bytes = Vec::with_capacity(array_len);
                for i in 0..array_len {
                    let value = field_type
                        .read_array_element(payload, *offset, i)
                        .unwrap_or(0.0);
                    let indexed_name = format!("{}[{}]", field_name, i);
                    let key = format!("{}:{}", msg_name, indexed_name);
                    metas.insert(key, value);

                    let byte = value as u8;
                    if byte != 0 {
                        bytes.push(byte);
                    }
                }
                // 存储字符串表示（使用特殊后缀 _str，值为字符串的哈希或长度，实际字符串在单独的map中）
                // 这里我们用一个特殊的负数编码字符串长度，UI层可以据此判断
                let str_key = format!("{}:{}", msg_name, field_name);
                // 存储字符串长度作为标记（负数表示这是char数组的字符串长度）
                metas.insert(str_key, -(bytes.len() as f64 + 0.5));
            } else if field_type.is_array() && array_len > 1 {
                for i in 0..array_len {
                    // MAVLink v2 尾部零截断：超出payload的字段值为0
                    let value = field_type
                        .read_array_element(payload, *offset, i)
                        .unwrap_or(0.0);
                    let indexed_name = format!("{}[{}]", field_name, i);
                    let key = format!("{}:{}", msg_name, indexed_name);
                    metas.insert(key, value);
                }
            } else {
                // MAVLink v2 尾部零截断：超出payload的字段值为0
                let value = field_type.read_value(payload, *offset).unwrap_or(0.0);
                let key = format!("{}:{}", msg_name, field_name);
                metas.insert(key, value);
            }
        }
    }

    /// 解析 payload 并写入 metas（包含枚举名称解析）
    pub fn parse_payload_with_enum_names(
        &self,
        msg_id: u32,
        payload: &[u8],
        metas: &mut HashMap<String, f64>,
        enum_metas: &mut HashMap<String, String>,
    ) {
        let msg_def = match self.message_metadata.get(&msg_id) {
            Some(def) => def,
            None => return,
        };

        let msg_name = &msg_def.name;
        let field_info = self.get_sorted_field_info_with_enum(msg_def);

        for (field_name, field_type, _units, enum_type, _is_ext, offset) in &field_info {
            let array_len = field_type.array_length();
            if field_type.is_array() && array_len > 1 {
                for i in 0..array_len {
                    if let Some(value) = field_type.read_array_element(payload, *offset, i) {
                        let indexed_name = format!("{}[{}]", field_name, i);
                        let key = format!("{}:{}", msg_name, indexed_name);
                        metas.insert(key.clone(), value);

                        // 如果有枚举类型，解析枚举名称
                        if let Some(enum_name) = enum_type {
                            if let Some(entry_name) =
                                self.get_enum_entry_name(enum_name, value as u64)
                            {
                                enum_metas.insert(key, entry_name.to_string());
                            }
                        }
                    }
                }
            } else {
                if let Some(value) = field_type.read_value(payload, *offset) {
                    let key = format!("{}:{}", msg_name, field_name);
                    metas.insert(key.clone(), value);

                    // 如果有枚举类型，解析枚举名称
                    if let Some(enum_name) = enum_type {
                        if let Some(entry_name) = self.get_enum_entry_name(enum_name, value as u64)
                        {
                            enum_metas.insert(key, entry_name.to_string());
                        }
                    }
                }
            }
        }
    }

    /// 获取消息定义
    pub fn get_message_def(&self, msg_id: u32) -> Option<&MessageDef> {
        self.message_metadata.get(&msg_id)
    }

    /// 获取消息名称
    pub fn get_message_name(&self, msg_id: u32) -> Option<&str> {
        self.message_metadata.get(&msg_id).map(|m| m.name.as_str())
    }

    /// 获取消息ID（通过名称）
    pub fn get_message_id(&self, name: &str) -> Option<u32> {
        self.name_to_id.get(name).copied()
    }

    /// 获取所有已加载的消息ID列表
    pub fn get_all_message_ids(&self) -> Vec<u32> {
        self.message_metadata.keys().copied().collect()
    }

    /// 构建 MAVLink v2 数据包
    pub fn build_mavlink_v2_packet(
        &self,
        msg_id: u32,
        sequence: u8,
        metas: &HashMap<String, f64>,
    ) -> Option<Vec<u8>> {
        let msg_def = self.message_metadata.get(&msg_id)?;
        let msg_name = msg_def.name.clone();
        let crc_extra = MavMessage::extra_crc(msg_id);

        let field_info = self.get_sorted_field_info(msg_def);
        let payload_size: usize = field_info.iter().map(|(_, ft, _, _)| ft.size()).sum();
        let mut payload = vec![0u8; payload_size];

        let mut found_any_value = false;

        for (field_name, field_type, _units, offset) in &field_info {
            let array_len = field_type.array_length();
            if field_type.is_array() && array_len > 1 {
                for i in 0..array_len {
                    let indexed_name = format!("{}[{}]", field_name, i);
                    let key = format!("{}:{}", msg_name, indexed_name);
                    if let Some(&value) = metas.get(&key) {
                        found_any_value = true;
                        field_type.write_array_element(&mut payload, *offset, i, value);
                    }
                }
            } else {
                let key = format!("{}:{}", msg_name, field_name);
                if let Some(&value) = metas.get(&key) {
                    found_any_value = true;
                    field_type.write_value(&mut payload, *offset, value);
                }
            }
        }

        if !found_any_value {
            return None;
        }

        let mut packet = Vec::with_capacity(12 + payload.len() + 2);
        packet.push(0xFD);
        packet.push(payload.len() as u8);
        packet.push(0);
        packet.push(0);
        packet.push(sequence);
        packet.push(self.system_id);
        packet.push(self.component_id);
        packet.push((msg_id & 0xFF) as u8);
        packet.push(((msg_id >> 8) & 0xFF) as u8);
        packet.push(((msg_id >> 16) & 0xFF) as u8);
        packet.extend_from_slice(&payload);

        let crc = Self::calculate_crc(&packet[1..], crc_extra);
        packet.push((crc & 0xFF) as u8);
        packet.push((crc >> 8) as u8);

        Some(packet)
    }

    /// 计算 MAVLink CRC
    fn calculate_crc(data: &[u8], crc_extra: u8) -> u16 {
        const CRC_INIT: u16 = 0xFFFF;
        let mut crc = CRC_INIT;
        for &byte in data {
            let tmp = (byte as u16) ^ (crc & 0xFF);
            let tmp = (tmp ^ (tmp << 4)) & 0xFF;
            crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
        }
        let tmp = (crc_extra as u16) ^ (crc & 0xFF);
        let tmp = (tmp ^ (tmp << 4)) & 0xFF;
        crc = (crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
        crc
    }
}

impl MavMapper {
    /// 设置文本字段值（自动映射字符串到字节数组）
    ///
    /// # 用法
    /// ```rust
    /// mapper.set_text_field(&mut metas, "PARAM_SET", "param_id", "MY_PARAM");
    /// ```
    ///
    /// 这会自动设置:
    /// - PARAM_SET:param_id[0] = 'M'
    /// - PARAM_SET:param_id[1] = 'Y'
    /// - PARAM_SET:param_id[2] = '_'
    /// - ... 以此类推，剩余位置填充 0
    pub fn set_text_field(
        &self,
        metas: &mut HashMap<String, f64>,
        msg_name: &str,
        field_name: &str,
        text: &str,
    ) -> Result<(), String> {
        // 获取字段的数组长度
        let array_len = self.get_field_array_length(msg_name, field_name)?;

        let text_bytes = text.as_bytes();
        if text_bytes.len() > array_len {
            return Err(format!(
                "文本长度 {} 超过字段 {}:{} 的最大长度 {}",
                text_bytes.len(),
                msg_name,
                field_name,
                array_len
            ));
        }

        // 设置文本字节
        for (i, &byte) in text_bytes.iter().enumerate() {
            let key = format!("{}:{}[{}]", msg_name, field_name, i);
            metas.insert(key, byte as f64);
        }

        // 剩余位置填充 0（null 终止符）
        for i in text_bytes.len()..array_len {
            let key = format!("{}:{}[{}]", msg_name, field_name, i);
            metas.insert(key, 0.0);
        }
        Ok(())
    }

    /// 从 metas 中读取文本字段值
    ///
    /// # 用法
    /// ```rust
    /// let param_id = mapper.get_text_field(&metas, "PARAM_SET", "param_id");
    /// // 返回 Some("MY_PARAM")
    /// ```
    pub fn get_text_field(
        &self,
        metas: &HashMap<String, f64>,
        msg_name: &str,
        field_name: &str,
    ) -> Option<String> {
        let array_len = self.get_field_array_length(msg_name, field_name).ok()?;

        let mut bytes = Vec::with_capacity(array_len);
        for i in 0..array_len {
            let key = format!("{}:{}[{}]", msg_name, field_name, i);
            match metas.get(&key) {
                Some(&v) => {
                    let byte = v as u8;
                    if byte == 0 {
                        break; // 遇到 null 终止
                    }
                    bytes.push(byte);
                }
                None => break,
            }
        }

        String::from_utf8(bytes).ok()
    }
    fn get_field_array_length(&self, msg_name: &str, field_name: &str) -> Result<usize, String> {
        let msg_id = self
            .name_to_id
            .get(msg_name)
            .ok_or_else(|| format!("消息 {} 不存在", msg_name))?;

        let msg_def = self
            .message_metadata
            .get(msg_id)
            .ok_or_else(|| format!("消息定义 {} 不存在", msg_name))?;

        let field = msg_def
            .fields
            .iter()
            .find(|f| f.name == field_name)
            .ok_or_else(|| format!("字段 {}:{} 不存在", msg_name, field_name))?;

        let field_type = FieldType::from_string(&field.field_type);
        Ok(field_type.array_length())
    }

    /// 判断字段是否为文本类型（char数组）
    pub fn is_text_field(&self, msg_name: &str, field_name: &str) -> bool {
        if let Some(msg_id) = self.name_to_id.get(msg_name) {
            if let Some(msg_def) = self.message_metadata.get(msg_id) {
                if let Some(field) = msg_def.fields.iter().find(|f| f.name == field_name) {
                    return field.field_type.starts_with("char[");
                }
            }
        }
        false
    }
}