use chan;
use futures::sync::mpsc;
use futures::{Future, Sink};
use miner::{Buffer, NonceData};
use ocl::GpuContext;
use ocl::{gpu_hash, gpu_transfer};
use reader::ReadReply;
use std::sync::Arc;

pub fn create_gpu_worker_task(
    benchmark: bool,
    rx_read_replies: chan::Receiver<ReadReply>,
    tx_empty_buffers: chan::Sender<Box<Buffer + Send>>,
    tx_nonce_data: mpsc::Sender<NonceData>,
    context_mu: Arc<GpuContext>,
) -> impl FnOnce() {
    move || {
        for read_reply in rx_read_replies {
            let mut buffer = read_reply.buffer;

            if read_reply.info.len == 0 || benchmark {
                tx_empty_buffers.send(buffer).unwrap();
                continue;
            }

            gpu_transfer(
                context_mu.clone(),
                buffer.get_gpu_buffers().unwrap(),
                *read_reply.info.gensig,
            );
            let result = gpu_hash(
                context_mu.clone(),
                read_reply.info.len / 64,
                buffer.get_gpu_data().as_ref().unwrap(),
                true,
            );
            let deadline = result.0;
            let offset = result.1;

            tx_nonce_data
                .clone()
                .send(NonceData {
                    height: read_reply.info.height,
                    base_target: read_reply.info.base_target,
                    deadline,
                    nonce: offset + read_reply.info.start_nonce,
                    reader_task_processed: read_reply.info.finished,
                    account_id: read_reply.info.account_id,
                })
                .wait()
                .expect("failed to send nonce data");

            tx_empty_buffers.send(buffer).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate ocl_core as core;
    use self::core::Event;
    use hex;
    use ocl::gpu_hash;
    use ocl::GpuContext;
    use std::sync::Arc;
    use std::u64;

    #[test]
    fn test_deadline_hashing() {
        let len: u64 = 16;
        let gensig =
            hex::decode("4a6f686e6e7946464d206861742064656e206772f6df74656e2050656e697321")
                .unwrap();
        let mut data: [u8; 64 * 16] = [0; 64 * 16];
        for i in 0..32 {
            data[i * 32..i * 32 + 32].clone_from_slice(&gensig);
        }

        let context = Arc::new(GpuContext::new(0, 0, 16, false));

        let buffer_gpu = unsafe {
            core::create_buffer::<_, u8>(&context.context, core::MEM_READ_ONLY, 64 * 16, None)
                .unwrap()
        };

        unsafe {
            core::enqueue_write_buffer(
                &context.queue_transfer_a,
                &context.gensig_gpu_a,
                true,
                0,
                &gensig,
                None::<Event>,
                None::<&mut Event>,
            )
            .unwrap();
        }

        unsafe {
            core::enqueue_write_buffer(
                &context.queue_transfer_a,
                &buffer_gpu,
                true,
                0,
                &data,
                None::<Event>,
                None::<&mut Event>,
            )
            .unwrap();
        }

        let result = gpu_hash(context.clone(), len as usize, &buffer_gpu, true);
        assert_eq!(18043101931632730606u64, result.0);
    }
}
