use chacha20poly1305::{
    aead::{Aead, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};

pub type NodeId = [u8; 16];
pub const PORT: u16 = 49735;
pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME_SAMPLES: usize = 320;
pub const FRAME_BYTES: usize = FRAME_SAMPLES * 2;
const MAGIC: &[u8; 4] = b"WR01";

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Hello,
    Bye,
    Start {
        talk: u64,
        chime: bool,
    },
    Ready {
        talk: u64,
        chime: bool,
    },
    Audio {
        talk: u64,
        sequence: u32,
        pcm: Vec<u8>,
    },
    End {
        talk: u64,
    },
}

pub struct Codec {
    cipher: XChaCha20Poly1305,
    pub node: NodeId,
    counter: u64,
}

impl Codec {
    pub fn new(key: &[u8; 32]) -> Result<Self, String> {
        let mut node = [0; 16];
        getrandom::getrandom(&mut node).map_err(|e| e.to_string())?;
        Ok(Self {
            cipher: XChaCha20Poly1305::new(key.into()),
            node,
            counter: 0,
        })
    }

    pub fn encode(&mut self, message: &Message) -> Vec<u8> {
        let mut plain = Vec::with_capacity(FRAME_BYTES + 13);
        match message {
            Message::Hello => plain.push(1),
            Message::Bye => plain.push(2),
            Message::Start { talk, chime } => {
                plain.push(3);
                plain.extend(talk.to_le_bytes());
                plain.push(u8::from(*chime));
            }
            Message::Ready { talk, chime } => {
                plain.push(4);
                plain.extend(talk.to_le_bytes());
                plain.push(u8::from(*chime));
            }
            Message::Audio {
                talk,
                sequence,
                pcm,
            } => {
                plain.push(5);
                plain.extend(talk.to_le_bytes());
                plain.extend(sequence.to_le_bytes());
                plain.extend(pcm);
            }
            Message::End { talk } => {
                plain.push(6);
                plain.extend(talk.to_le_bytes());
            }
        }
        let mut nonce = [0; 24];
        nonce[..16].copy_from_slice(&self.node);
        nonce[16..].copy_from_slice(&self.counter.to_le_bytes());
        self.counter = self
            .counter
            .checked_add(1)
            .expect("packet counter exhausted");
        let encrypted = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: MAGIC,
                },
            )
            .expect("valid packet size");
        let mut packet = Vec::with_capacity(28 + encrypted.len());
        packet.extend(MAGIC);
        packet.extend(nonce);
        packet.extend(encrypted);
        packet
    }

    pub fn decode(&self, packet: &[u8]) -> Option<(NodeId, u64, Message)> {
        if packet.len() < 45 || packet.len() > 1024 || &packet[..4] != MAGIC {
            return None;
        }
        let node: NodeId = packet[4..20].try_into().ok()?;
        let counter = u64::from_le_bytes(packet[20..28].try_into().ok()?);
        let data = self
            .cipher
            .decrypt(
                XNonce::from_slice(&packet[4..28]),
                Payload {
                    msg: &packet[28..],
                    aad: MAGIC,
                },
            )
            .ok()?;
        let message = match (data.first()?, data.len()) {
            (1, 1) => Message::Hello,
            (2, 1) => Message::Bye,
            (3, 10) if data[9] <= 1 => Message::Start {
                talk: u64::from_le_bytes(data[1..9].try_into().ok()?),
                chime: data[9] == 1,
            },
            (4, 10) if data[9] <= 1 => Message::Ready {
                talk: u64::from_le_bytes(data[1..9].try_into().ok()?),
                chime: data[9] == 1,
            },
            (5, n) if n == 13 + FRAME_BYTES => Message::Audio {
                talk: u64::from_le_bytes(data[1..9].try_into().ok()?),
                sequence: u32::from_le_bytes(data[9..13].try_into().ok()?),
                pcm: data[13..].to_vec(),
            },
            (6, 9) => Message::End {
                talk: u64::from_le_bytes(data[1..9].try_into().ok()?),
            },
            _ => return None,
        };
        Some((node, counter, message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_messages_roundtrip_and_packets_fit_in_one_datagram() {
        let mut sender = Codec::new(&[7; 32]).unwrap();
        let receiver = Codec::new(&[7; 32]).unwrap();
        let messages = [
            Message::Hello,
            Message::Bye,
            Message::Start {
                talk: 15,
                chime: true,
            },
            Message::Ready {
                talk: 15,
                chime: true,
            },
            Message::Audio {
                talk: 15,
                sequence: 8,
                pcm: vec![42; FRAME_BYTES],
            },
            Message::End { talk: 15 },
        ];
        for (index, message) in messages.into_iter().enumerate() {
            let packet = sender.encode(&message);
            assert!(packet.len() < 1200);
            let (node, counter, decoded) = receiver.decode(&packet).unwrap();
            assert_eq!(node, sender.node);
            assert_eq!(counter, index as u64);
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn wrong_room_tampering_and_truncation_are_rejected() {
        let mut sender = Codec::new(&[1; 32]).unwrap();
        let receiver = Codec::new(&[2; 32]).unwrap();
        let mut packet = sender.encode(&Message::Hello);
        assert!(receiver.decode(&packet).is_none());
        packet[30] ^= 1;
        assert!(sender.decode(&packet).is_none());
        for size in 0..packet.len() {
            assert!(sender.decode(&packet[..size]).is_none());
        }
    }
}
