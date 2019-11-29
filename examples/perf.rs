#![cfg_attr(all(target_arch = "x86_64", feature = "nightly_perf_example"), feature(asm))]

use spsc_bip_buffer::bip_buffer_from;

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
#[cfg(all(target_arch = "x86_64", feature = "nightly_perf_example"))]
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

    // usage: ./perf <core_id of sender> <core_id of receiver> <message length in bytes>
    //               <queue size in bytes> <number of repetitions>
    //               <true/false whether to check message on receiver>
    // example: cargo run --release --example perf -- 4 5 255 16384 true

    let sender_core_id: usize = args.next().unwrap().parse().unwrap();
    let receiver_core_id: usize = args.next().unwrap().parse().unwrap();
    let length: usize = args.next().unwrap().parse().unwrap();
    let queue_size: usize = args.next().unwrap().parse().unwrap();
    let repetitions: usize = args.next().unwrap().parse().unwrap();
    let test_correctness: bool = args.next().unwrap().parse().unwrap();

    let mut core_ids: Vec<_> = core_affinity::get_core_ids().unwrap().into_iter().map(Some).collect();
    let sender_core_id = core_ids[sender_core_id].take().unwrap();
    let receiver_core_id = core_ids[receiver_core_id].take().unwrap();

    let start = std::time::Instant::now();

    let (mut writer, mut reader) = bip_buffer_from(vec![0u8; queue_size].into_boxed_slice());
    let sender = ::std::thread::spawn(move || {
        core_affinity::set_for_current(sender_core_id);
        #[cfg(all(target_arch = "x86_64", feature = "nightly_perf_example"))]
        let mut sender_hist = streaming_harness_hdrhist::HDRHist::new();

        let mut msg = vec![0u8; length];
        for _ in 0..repetitions {
            for round in 0..128u8 {
                for i in 0..length {
                    msg[i as usize] = round;
                }
                #[cfg(all(target_arch = "x86_64", feature = "nightly_perf_example"))]
                let start = tsc_ticks();
                writer.spin_reserve(length as usize).copy_from_slice(&msg[..length as usize]);
                #[cfg(all(target_arch = "x86_64", feature = "nightly_perf_example"))]
                {
                    let stop = tsc_ticks();
                    sender_hist.add_value(stop - start);
                }
            }
        }

        eprintln!("sender done");
        #[cfg(all(target_arch = "x86_64", feature = "nightly_perf_example"))]
        let ret = sender_hist;
        #[cfg(not(all(target_arch = "x86_64", feature = "nightly_perf_example")))]
        let ret = ();
        ret
    });
    let receiver = ::std::thread::spawn(move || {
        core_affinity::set_for_current(receiver_core_id);
        let mut msgs_received: usize = 0;

        let mut msg = vec![0u8; length];
        for _ in 0..repetitions {
            for round in 0..128u8 {
                let recv_msg = loop {
                    let valid = reader.valid();
                    if valid.len() < length { continue; }
                    break valid;
                };
                if test_correctness {
                    for i in 0..length {
                        msg[i as usize] = round;
                    }
                    assert_eq!(&recv_msg[..length], &msg[..]);
                }
                assert!(reader.consume(length));
                msgs_received += 1;
            }
        }

        let elapsed = start.elapsed();
        let bytes_received = msgs_received * length;

        eprintln!("receiver done");
        let nanos = elapsed.as_secs() * 1_000_000_000 + (elapsed.subsec_nanos() as u64);
        let bytes_per_sec = bytes_received as f64 / ((nanos as f64) / 1_000_000_000f64);
        let msgs_per_sec = msgs_received as f64 / ((nanos as f64) / 1_000_000_000f64);
        eprintln!("elapsed (nanos)\tbytes\tmsgs\tbytes/s\tmsgs/s");
        println!("{}\t{}\t{}\t{}\t{}", nanos, bytes_received, msgs_received, bytes_per_sec, msgs_per_sec);
    });
    #[cfg(all(target_arch = "x86_64", feature = "nightly_perf_example"))]
    {
        let hist = sender.join().unwrap();
        eprintln!("reserve and send:\n{}", hist.summary_string());
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "nightly_perf_example")))]
    sender.join().unwrap();
    receiver.join().unwrap();
}
