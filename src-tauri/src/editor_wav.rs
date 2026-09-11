//! Recognize ordinary PCM/float WAVs that can be transferred without FFmpeg.
pub struct WavInfo { pub codec: &'static str, pub decoded_bytes: usize }
pub fn inspect(bytes: &[u8]) -> Option<WavInfo> {
    let u16at = |at| Some(u16::from_le_bytes(bytes.get(at..at+2)?.try_into().ok()?));
    let u32at = |at| Some(u32::from_le_bytes(bytes.get(at..at+4)?.try_into().ok()?));
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" { return None; }
    let limit = (u32at(4)? as usize).checked_add(8)?;
    if limit > bytes.len() { return None; }
    let (mut format, mut channels, mut rate, mut bits, mut align, mut size) = (0,0,0,0,0,0);
    let mut at = 12usize;
    while at.checked_add(8)? <= limit {
        let n = u32at(at+4)? as usize;
        let end = at.checked_add(8)?.checked_add(n)?;
        if end > limit { return None; }
        match &bytes[at..at+4] {
            b"fmt " => {
                if n < 16 { return None; }
                format = u16at(at+8)?; channels = u16at(at+10)?; rate = u32at(at+12)?;
                align = u16at(at+20)?; bits = u16at(at+22)?;
                if format == 65534 {
                    if n < 40 || u16at(at+24)? < 22 || u16at(at+26)? != bits { return None; }
                    if bytes.get(at+34..at+48)? != [0,0,0,0,16,0,128,0,0,170,0,56,155,113] { return None; }
                    format = u16at(at+32)?;
                }
            }
            b"data" => { if size != 0 { return None; } size = n; }
            _ => {}
        }
        at = end.checked_add(n % 2)?;
    }
    if !(1..=2).contains(&channels) || rate == 0 || size == 0 { return None; }
    let codec = match (format, bits) {
        (1,8) => "pcm_u8", (1,16) => "pcm_s16le", (1,24) => "pcm_s24le", (1,32) => "pcm_s32le",
        (3,32) => "pcm_f32le", (3,64) => "pcm_f64le", _ => return None,
    };
    if align != channels * (bits / 8) || size % align as usize != 0 { return None; }
    Some(WavInfo { codec, decoded_bytes: (size / (bits as usize / 8)).checked_mul(4)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pcm_formats_and_truncation() {
        for (format,bits,codec) in [(1,8,"pcm_u8"),(1,16,"pcm_s16le"),(1,24,"pcm_s24le"),(1,32,"pcm_s32le"),(3,32,"pcm_f32le"),(3,64,"pcm_f64le")] {
            let channels = 2u16; let align = channels * bits / 8;
            let mut bytes = vec![0u8;44+align as usize*10];
            bytes[..4].copy_from_slice(b"RIFF"); let n=bytes.len() as u32-8; bytes[4..8].copy_from_slice(&n.to_le_bytes());
            bytes[8..16].copy_from_slice(b"WAVEfmt "); bytes[16..20].copy_from_slice(&16u32.to_le_bytes());
            bytes[20..22].copy_from_slice(&(format as u16).to_le_bytes()); bytes[22..24].copy_from_slice(&channels.to_le_bytes());
            bytes[24..28].copy_from_slice(&48000u32.to_le_bytes()); bytes[32..34].copy_from_slice(&align.to_le_bytes());
            bytes[34..36].copy_from_slice(&bits.to_le_bytes()); bytes[36..40].copy_from_slice(b"data");
            bytes[40..44].copy_from_slice(&(align as u32*10).to_le_bytes());
            let info=inspect(&bytes).unwrap(); assert_eq!(info.codec,codec); assert_eq!(info.decoded_bytes,80);
            let mut truncated=bytes.clone(); truncated.pop(); assert!(inspect(&truncated).is_none());
            bytes.splice(36..36, [0u8;24]);
            let n=bytes.len() as u32-8;bytes[4..8].copy_from_slice(&n.to_le_bytes());
            bytes[16..20].copy_from_slice(&40u32.to_le_bytes());bytes[20..22].copy_from_slice(&65534u16.to_le_bytes());
            bytes[36..38].copy_from_slice(&22u16.to_le_bytes());bytes[38..40].copy_from_slice(&bits.to_le_bytes());
            bytes[44..46].copy_from_slice(&(format as u16).to_le_bytes());
            bytes[46..60].copy_from_slice(&[0,0,0,0,16,0,128,0,0,170,0,56,155,113]);
            assert_eq!(inspect(&bytes).unwrap().codec,codec);
            bytes[59]=0;assert!(inspect(&bytes).is_none());
        }
    }
}
