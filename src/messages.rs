
pub fn build_connect_request(target:&str) -> Vec<u8> {
    let bytes = target.as_bytes();
    let length = bytes.len();
    let mut buf = Vec::with_capacity(bytes.len() + 4);
    let length_bytes = (length as u32).to_be_bytes();
    buf.extend_from_slice(&length_bytes);
    buf.extend_from_slice(bytes);
    return buf;
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