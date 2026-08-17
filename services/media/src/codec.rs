#![allow(dead_code)]
// G.711 μ-law codec
// 20 ms @ 8000 Hz = 160 samples per frame.

pub const SAMPLES_PER_FRAME: usize = 160;

// Decode table: u8 -> i16
const MULAW_TO_PCM: [i16; 256] = {
    let mut table = [0i16; 256];
    let mut i = 0;
    while i < 256 {
        let mulaw = !i as u8;
        let sign = (mulaw & 0x80) != 0;
        let exponent = ((mulaw & 0x70) >> 4) as i32;
        let mantissa = (mulaw & 0x0f) as i32;

        let mut sample = (mantissa << 3) + 132;
        sample <<= exponent;
        sample -= 132;

        let pcm = if sign { -sample } else { sample };
        // Clip to i16 range just in case, though standard μ-law max is 8031.
        table[i] = pcm as i16;
        i += 1;
    }
    table
};

#[inline(always)]
pub fn decode_mulaw_byte(mulaw: u8) -> i16 {
    MULAW_TO_PCM[mulaw as usize]
}

#[inline(always)]
pub fn encode_mulaw_sample(mut pcm: i16) -> u8 {
    let sign = if pcm < 0 {
        pcm = -pcm;
        0x80
    } else {
        0x00
    };

    let pcm = pcm as i32;
    // Clip to max valid magnitude for μ-law
    let pcm = if pcm > 32635 { 32635 } else { pcm };
    let pcm = pcm + 132;

    let mut exponent = 7;
    let mut mask = 0x4000;
    while (pcm & mask) == 0 && exponent > 0 {
        exponent -= 1;
        mask >>= 1;
    }

    let mantissa = (pcm >> (exponent + 3)) & 0x0F;
    let mulaw = sign | (exponent << 4) | mantissa;

    !(mulaw as u8)
}

pub fn decode_frame(mulaw_bytes: &[u8]) -> Vec<i16> {
    mulaw_bytes.iter().copied().map(decode_mulaw_byte).collect()
}

pub fn encode_frame(pcm_samples: &[i16]) -> Vec<u8> {
    pcm_samples.iter().copied().map(encode_mulaw_sample).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        for original in -8000..=8000 {
            let mulaw = encode_mulaw_sample(original);
            let decoded = decode_mulaw_byte(mulaw);
            
            // μ-law is lossy, so we check if it's within tolerance.
            // Max error in μ-law is roughly 3% of the value.
            let diff = (original - decoded).abs();
            let tolerance = 8.max((original.abs() as f32 * 0.05) as i16);
            
            assert!(
                diff <= tolerance,
                "orig: {}, mulaw: {}, dec: {}, diff: {} > tol: {}",
                original, mulaw, decoded, diff, tolerance
            );
        }
    }

    #[test]
    fn test_frame_size() {
        let pcm = vec![0i16; SAMPLES_PER_FRAME];
        let encoded = encode_frame(&pcm);
        assert_eq!(encoded.len(), SAMPLES_PER_FRAME);
        
        let decoded = decode_frame(&encoded);
        assert_eq!(decoded.len(), SAMPLES_PER_FRAME);
    }
}
