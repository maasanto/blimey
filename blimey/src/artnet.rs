//! Art-Net output, per Artistic Licence's "Art-Net 4" specification: one ArtDmx packet per
//! universe per frame, over UDP. Used as a preview mirror (BlenderDMX, docs/blenderdmx.md)
//! and as the real output for pixel grids on Art-Net universes.

use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use blimey_engine::{ArtNetFrames, DMX_CHANNELS, DmxSink, Frame};

pub const PORT: u16 = 6454;
/// Port-address is 15 bits: Net (7), Sub-Net (4), Universe (4).
pub const MAX_UNIVERSE: u16 = blimey_engine::rig::MAX_ARTNET_UNIVERSE;
const ART_DMX_OPCODE: u16 = 0x5000;
const PROTOCOL_VERSION: u16 = 14;

/// One ArtDmx packet. `universe` is the 15-bit port-address; the data is padded to an even
/// length of at least 2, as the specification requires.
pub fn art_dmx_packet(universe: u16, sequence: u8, channels: &[u8]) -> Vec<u8> {
    assert!(
        universe <= MAX_UNIVERSE,
        "Art-Net universe {universe} > {MAX_UNIVERSE}"
    );
    assert!(channels.len() <= DMX_CHANNELS, "a universe is 512 channels");
    let length = channels.len().max(2).next_multiple_of(2);
    let mut packet = Vec::with_capacity(18 + length);
    packet.extend(b"Art-Net\0");
    packet.extend(ART_DMX_OPCODE.to_le_bytes());
    packet.extend(PROTOCOL_VERSION.to_be_bytes());
    packet.push(sequence);
    packet.push(0); // Physical: informational only.
    packet.extend(universe.to_le_bytes()); // SubUni, then Net.
    packet.extend(u16::try_from(length).unwrap().to_be_bytes());
    packet.extend(channels);
    packet.resize(18 + length, 0);
    packet
}

/// The universe (15-bit port-address) and channels of an ArtDmx packet; `None` for any other
/// packet.
pub fn parse_art_dmx(packet: &[u8]) -> Option<(u16, &[u8])> {
    let header = packet.get(..18)?;
    if header[..8] != *b"Art-Net\0" || header[8..10] != ART_DMX_OPCODE.to_le_bytes() {
        return None;
    }
    let universe = u16::from_le_bytes([header[14], header[15]]);
    let length = usize::from(u16::from_be_bytes([header[16], header[17]])).min(DMX_CHANNELS);
    Some((universe, packet.get(18..18 + length)?))
}

/// Accepts `127.0.0.1` (port 6454) or `2.0.0.10:6454`.
pub fn parse_node(text: &str) -> Result<SocketAddrV4, String> {
    text.parse()
        .or_else(|_| {
            text.parse::<Ipv4Addr>()
                .map(|ip| SocketAddrV4::new(ip, PORT))
        })
        .map_err(|_| {
            format!("expected an IPv4 address such as 127.0.0.1 or 2.0.0.10:6454, got {text:?}")
        })
}

/// Sends to one node, unicast or broadcast. As a `DmxSink` it mirrors the engine's frame on
/// `mirror_universe`, if any, and sends the pixel grids' universes.
pub struct ArtNet {
    socket: UdpSocket,
    node: SocketAddrV4,
    mirror_universe: Option<u16>,
    /// The last sequence number sent on each universe, 1 to 255: receivers reorder packets per
    /// universe, so each counts on its own. 0 would tell them not to reorder.
    sequences: BTreeMap<u16, u8>,
    is_failure_reported: bool,
}

impl ArtNet {
    pub fn new(node: SocketAddrV4, mirror_universe: Option<u16>) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        // Nodes are often addressed by broadcast, e.g. 2.255.255.255.
        socket.set_broadcast(true)?;
        Ok(Self {
            socket,
            node,
            mirror_universe,
            sequences: BTreeMap::new(),
            is_failure_reported: false,
        })
    }

    /// Never fails: a send error is reported once on stderr and the render loop keeps going.
    pub fn send_universe(&mut self, universe: u16, channels: &[u8]) {
        let sequence = self.sequences.entry(universe).or_default();
        *sequence = *sequence % 255 + 1;
        let packet = art_dmx_packet(universe, *sequence, channels);
        match self.socket.send_to(&packet, self.node) {
            Ok(_) => self.is_failure_reported = false,
            Err(error) if !self.is_failure_reported => {
                eprintln!(
                    "artnet: cannot send to {}: {error}; still trying",
                    self.node
                );
                self.is_failure_reported = true;
            }
            Err(_) => {}
        }
    }
}

impl DmxSink for ArtNet {
    fn send(&mut self, _at: Duration, frame: &Frame) -> io::Result<()> {
        if let Some(universe) = self.mirror_universe {
            self.send_universe(universe, frame);
        }
        Ok(())
    }

    fn send_artnet(&mut self, universes: &ArtNetFrames) {
        for (universe, frame) in universes {
            self.send_universe(*universe, frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_has_header_opcode_version_and_big_endian_length() {
        let packet = art_dmx_packet(0, 7, &[0xAB; DMX_CHANNELS]);

        assert_eq!(packet.len(), 18 + 512);
        assert_eq!(&packet[..8], b"Art-Net\0");
        assert_eq!(packet[8..10], [0x00, 0x50], "OpDmx 0x5000, low byte first");
        assert_eq!(
            packet[10..12],
            [0, 14],
            "protocol version 14, high byte first"
        );
        assert_eq!(packet[12], 7, "sequence");
        assert_eq!(packet[16..18], [0x02, 0x00], "length 512, high byte first");
        assert_eq!(packet[18], 0xAB, "channel 1");
    }

    #[test]
    fn universe_splits_into_sub_uni_and_net() {
        let packet = art_dmx_packet(0x1234, 1, &[0; 2]);

        assert_eq!(packet[14], 0x34, "SubUni: Sub-Net 3, Universe 4");
        assert_eq!(packet[15], 0x12, "Net 0x12");
    }

    #[test]
    fn data_is_padded_to_an_even_length_of_at_least_two() {
        let odd = art_dmx_packet(0, 1, &[9, 8, 7]);
        let empty = art_dmx_packet(0, 1, &[]);

        assert_eq!(odd[16..], [0, 4, 9, 8, 7, 0]);
        assert_eq!(empty[16..], [0, 2, 0, 0]);
    }

    #[test]
    fn parse_reads_back_what_the_packet_carries_and_skips_other_opcodes() {
        let packet = art_dmx_packet(0x0123, 9, &[1, 2, 3]);
        let mut poll = packet.clone();
        poll[8..10].copy_from_slice(&0x2000u16.to_le_bytes());

        assert_eq!(parse_art_dmx(&packet), Some((0x0123, &[1, 2, 3, 0][..])));
        assert_eq!(parse_art_dmx(&poll), None, "ArtPoll is not DMX");
        assert_eq!(parse_art_dmx(&packet[..19]), None, "cut short");
    }

    #[test]
    #[should_panic(expected = "32768")]
    fn universe_beyond_15_bits_is_refused() {
        art_dmx_packet(0x8000, 1, &[0; 2]);
    }

    #[test]
    fn sequence_counts_1_to_255_then_wraps_past_0() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut sender = ArtNet::new(local_address(&receiver), Some(0)).unwrap();
        let mut buffer = [0; 600];

        let sequences: Vec<u8> = (0..257)
            .map(|_| {
                sender.send_universe(0, &[0; 2]);
                receiver.recv(&mut buffer).unwrap();
                buffer[12]
            })
            .collect();

        assert_eq!(sequences[..2], [1, 2]);
        assert_eq!(sequences[254..], [255, 1, 2]);
    }

    #[test]
    fn each_universe_counts_its_own_sequence() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut sender = ArtNet::new(local_address(&receiver), None).unwrap();
        let mut buffer = [0; 600];

        let sent: Vec<(u16, u8)> = [0, 1, 0, 1, 1]
            .into_iter()
            .map(|universe| {
                sender.send_universe(universe, &[0; 2]);
                receiver.recv(&mut buffer).unwrap();
                (u16::from_le_bytes([buffer[14], buffer[15]]), buffer[12])
            })
            .collect();

        assert_eq!(sent, [(0, 1), (1, 1), (0, 2), (1, 2), (1, 3)]);
    }

    #[test]
    fn without_a_mirror_universe_the_engine_frame_is_not_sent() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut sender = ArtNet::new(local_address(&receiver), None).unwrap();

        sender.send(Duration::ZERO, &[1; DMX_CHANNELS]).unwrap();

        assert!(receiver.recv(&mut [0; 600]).is_err());
    }

    #[test]
    fn several_universes_arrive_on_localhost_and_decode_per_universe() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut sender = ArtNet::new(local_address(&receiver), Some(0)).unwrap();
        let mut frame = [0; DMX_CHANNELS];
        frame[0] = 10;
        frame[511] = 11;

        sender.send(Duration::ZERO, &frame).unwrap();
        sender.send_universe(1, &[20; 384]);
        sender.send_universe(0x0110, &[30; 3]);

        let mut universes: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
        let mut buffer = [0; 600];
        for _ in 0..3 {
            let size = receiver.recv(&mut buffer).unwrap();
            let packet = &buffer[..size];
            let universe = u16::from_le_bytes([packet[14], packet[15]]);
            let length = usize::from(u16::from_be_bytes([packet[16], packet[17]]));
            assert_eq!(size, 18 + length);
            universes.insert(universe, packet[18..].to_vec());
        }

        assert_eq!(
            universes.keys().copied().collect::<Vec<_>>(),
            [0, 1, 0x0110]
        );
        assert_eq!(universes[&0], frame, "the mirrored frame, all 512 channels");
        assert_eq!(universes[&1], [20; 384]);
        assert_eq!(universes[&0x0110], [30, 30, 30, 0]);
    }

    #[test]
    fn node_defaults_to_the_art_net_port() {
        assert_eq!(
            parse_node("127.0.0.1"),
            Ok(SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT))
        );
        assert_eq!(parse_node("2.0.0.10:7000").unwrap().port(), 7000);
        assert!(
            parse_node("localhost")
                .unwrap_err()
                .contains("\"localhost\"")
        );
    }

    fn local_address(socket: &UdpSocket) -> SocketAddrV4 {
        match socket.local_addr().unwrap() {
            std::net::SocketAddr::V4(address) => address,
            other => panic!("bound to IPv4, got {other}"),
        }
    }
}
