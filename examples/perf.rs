use spsc_bip_buffer::bip_buffer;

const REPETITIONS: usize = 1<<18;

fn main() {
    let mut args = ::std::env::args();
    let _ = args.next();
    let sender_core_id: usize = args.next().unwrap().parse().unwrap();
    let receiver_core_id: usize = args.next().unwrap().parse().unwrap();
    let test_correctness: bool = args.next().unwrap().parse().unwrap();

    let mut core_ids: Vec<_> = core_affinity::get_core_ids().unwrap().into_iter().map(Some).collect();
    let sender_core_id = core_ids[sender_core_id].take().unwrap();
    let receiver_core_id = core_ids[receiver_core_id].take().unwrap();

    let start = std::time::Instant::now();

    const LENGTH: usize = 255;
    let (mut writer, mut reader) = bip_buffer(vec![0u8; 16<<10].into_boxed_slice());
    let sender = ::std::thread::spawn(move || {
        core_affinity::set_for_current(sender_core_id);

        let mut msg = [0u8; LENGTH];
        for _ in 0..REPETITIONS {
            for round in 0..128u8 {
                let length: u8 = LENGTH as u8;
                msg[0] = length;
                for i in 1..length {
                    msg[i as usize] = round;
                }
                writer.spin_reserve(length as usize).copy_from_slice(&msg[..length as usize]);
            }
        }

        eprintln!("sender done");
    });
    let receiver = ::std::thread::spawn(move || {
        core_affinity::set_for_current(receiver_core_id);
        let mut msgs_received: usize = 0;

        let mut msg = [0u8; LENGTH];
        for _ in 0..REPETITIONS {
            for round in 0..128u8 {
                let recv_msg = loop {
                    let valid = reader.valid();
                    if valid.len() < LENGTH { continue; }
                    break valid;
                };
                if test_correctness {
                    msg[0] = LENGTH as u8;
                    for i in 1..LENGTH {
                        msg[i as usize] = round;
                    }
                    assert_eq!(&recv_msg[..LENGTH], &msg[..]);
                }
                assert!(reader.consume(LENGTH as usize));
                msgs_received += 1;
            }
        }

        let elapsed = start.elapsed();
        let bytes_received = msgs_received * LENGTH;

        eprintln!("receiver done");
        let nanos = elapsed.as_secs() * 1_000_000_000 + (elapsed.subsec_nanos() as u64);
        let bytes_per_sec = bytes_received as f64 / ((nanos as f64) / 1_000_000_000f64);
        let msgs_per_sec = msgs_received as f64 / ((nanos as f64) / 1_000_000_000f64);
        println!("{}\t{}\t{}\t{}\t{}", nanos, bytes_received, msgs_received, bytes_per_sec, msgs_per_sec);
    });
    sender.join().unwrap();
    receiver.join().unwrap();
}
