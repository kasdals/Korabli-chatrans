// 临时工具：把存档回放解成原始包流（Blowfish + zlib 容器）。
use std::io::Read;

use blowfish::Blowfish;
use cipher::{BlockDecrypt, KeyInit};

const REPLAY_HEADER: [u8; 4] = [0x12, 0x32, 0x34, 0x11];
const BLOWFISH_KEY: [u8; 16] = [
    0x29, 0xB7, 0xC9, 0x09, 0x38, 0x3F, 0x84, 0x88,
    0xFA, 0x98, 0xEC, 0x4E, 0x13, 0x19, 0x79, 0xFB,
];

fn le_u32(data: &[u8], at: usize) -> usize {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let contents = std::fs::read(&args[1]).expect("read src");
    assert_eq!(&contents[0..4], &REPLAY_HEADER, "bad replay header");
    let slides_count = le_u32(&contents, 4);
    let meta_len = le_u32(&contents, 8);
    let mut i = 12 + meta_len;
    for _ in 0..slides_count.saturating_sub(1) {
        i += 4 + le_u32(&contents, i);
    }
    let remaining = &contents[i..];
    let blowfish: Blowfish = Blowfish::new_from_slice(&BLOWFISH_KEY).unwrap();
    let encrypted = &remaining[8..];
    let mut decrypted = vec![0u8; encrypted.len()];
    let mut previous: [u8; 8] = [0; 8];
    for (in_block, out_block) in encrypted.chunks(8).zip(decrypted.chunks_mut(8)) {
        blowfish.decrypt_block_b2b(in_block.into(), out_block.into());
        for (k, byte) in out_block.iter_mut().enumerate() {
            *byte ^= previous[k];
        }
        previous.copy_from_slice(out_block);
    }
    let mut deflater = flate2::read::ZlibDecoder::new(&decrypted[..]);
    let mut packet_data = vec![];
    deflater.read_to_end(&mut packet_data).expect("inflate");
    std::fs::write(&args[2], &packet_data).expect("write");
    println!("packet_data bytes = {}", packet_data.len());
}
