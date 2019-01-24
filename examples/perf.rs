use spsc_bip_buffer::bip_buffer;

const REPETITIONS: usize = 1<<18;

fn main() {
    use rand::Rng;

    let mut args = ::std::env::args();
    let _ = args.next();
    let sender_core_id: usize = args.next().unwrap().parse().unwrap();
    let receiver_core_id: usize = args.next().unwrap().parse().unwrap();

    let mut core_ids: Vec<_> = core_affinity::get_core_ids().unwrap().into_iter().map(Some).collect();
    let sender_core_id = core_ids[sender_core_id].take().unwrap();
    let receiver_core_id = core_ids[receiver_core_id].take().unwrap();

    let start = std::time::Instant::now();

    const MAX_LENGTH: usize = 255;
    let (mut writer, mut reader) = bip_buffer(vec![0u8; 16<<10].into_boxed_slice());
    let sender = ::std::thread::spawn(move || {
        core_affinity::set_for_current(sender_core_id);

        let mut rng = rand::thread_rng();
        let mut msg = [0u8; MAX_LENGTH];
        for _ in 0..REPETITIONS {
            for round in 0..128u8 {
                let length: u8 = rng.gen_range(1, MAX_LENGTH as u8);
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
        let mut bytes_received = 0;
        let mut msgs_received: usize = 0;

        let mut msg = [0u8; MAX_LENGTH];
        for _ in 0..REPETITIONS {
            for round in 0..128u8 {
                let msg_len = loop {
                    let valid = reader.valid();
                    if valid.len() < 1 { continue; }
                    break valid[0] as usize;
                };
                let recv_msg = loop {
                    let valid = reader.valid();
                    if valid.len() < msg_len { continue; }
                    break valid;
                };
                msg[0] = msg_len as u8;
                for i in 1..msg_len {
                    msg[i as usize] = round;
                }
                assert_eq!(&recv_msg[..msg_len], &msg[..msg_len]);
                assert!(reader.consume(msg_len as usize));
                bytes_received += msg_len;
                msgs_received += 1;
            }
        }

        let elapsed = start.elapsed();

        eprintln!("receiver done");
        let nanos = elapsed.as_secs() * 1_000_000_000 + (elapsed.subsec_nanos() as u64);
        let bytes_per_sec = bytes_received as f64 / ((nanos as f64) / 1_000_000_000f64);
        let msgs_per_sec = msgs_received as f64 / ((nanos as f64) / 1_000_000_000f64);
        println!("{}\t{}\t{}\t{}\t{}", nanos, bytes_received, msgs_received, bytes_per_sec, msgs_per_sec);
    });
    sender.join().unwrap();
    receiver.join().unwrap();
}
