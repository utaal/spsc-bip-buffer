#![feature(asm)]

use spsc_bip_buffer::bip_buffer;

const REPETITIONS: usize = 1<<20;

// ==================================

// This section is from https://github.com/valarauca/amd64_timer
//
// Copyright (c) 2016 Willliam Cody Laeder
// Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
// 
// The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
// 
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.


#[no_mangle]
#[inline(never)]
#[cfg(target_arch = "x86_64")]
pub fn tsc_ticks() -> u64 {
    let mask = 0xFFFFFFFFu64;
    let high: u64;
    let low: u64;
    unsafe {
        asm!("lfence;rdtsc"
            : "={edx}"(high), "={eax}"(low)
            :
            : "rdx", "rax"
            : "volatile"
        );
    }
    ((high)<<32) | (mask&low)
}

// ==================================

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
        let mut sender_hist = streaming_harness_hdrhist::HDRHist::new();

        let mut msg = [0u8; LENGTH];
        for _ in 0..REPETITIONS {
            for round in 0..128u8 {
                let length: u8 = LENGTH as u8;
                msg[0] = length;
                for i in 1..length {
                    msg[i as usize] = round;
                }
                let start = tsc_ticks();
                writer.spin_reserve(length as usize).copy_from_slice(&msg[..length as usize]);
                let stop = tsc_ticks();
                sender_hist.add_value(stop - start);
            }
        }

        eprintln!("sender done");
        sender_hist
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
                assert!(reader.consume(LENGTH));
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
    let hist = sender.join().unwrap();
    eprintln!("{}", hist.summary_string());
    receiver.join().unwrap();
}
