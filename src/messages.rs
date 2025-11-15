use std::collections::HashMap;


pub fn build_connect_request(target:&str, source_ip:&str) -> HashMap<String, String> {
    let mut data = HashMap::<String, String>::new();
    data.insert("target".to_string(), target.to_string());
    data.insert("source_ip".to_string(), source_ip.to_string());
    return data;
}

pub fn build_connect_response(ok:bool) -> Vec<u8> {
    let mut buf = Vec::new();
    let length:u32 = 1 as u32;
    let length_bytes = length.to_be_bytes();
    buf.extend_from_slice(&length_bytes);
    buf.push(if ok { 0x00 } else { 0x01 });
    return buf;
}

pub fn is_connect_success(packet:&[u8]) -> bool {
    return packet[0] == 0x00;
}