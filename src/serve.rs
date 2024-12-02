use bitcoincore_rpc::{Client, Result as ClientResult, RpcApi};
use bitcoincore_rpc_json::StringOrStringArray;
use hyper::{header::CONTENT_TYPE, Body, Method, Request, Response};
use prometheus::{Encoder, TextEncoder};
use std::{net::SocketAddr, sync::Arc};

use crate::metrics::{
    BITCOIN_BANNED_UNTIL, BITCOIN_BAN_CREATED, BITCOIN_BLOCKS, BITCOIN_CONN_IN, BITCOIN_CONN_OUT,
    BITCOIN_DIFFICULTY, BITCOIN_HASHPS, BITCOIN_HASHPS_1, BITCOIN_LATEST_BLOCK_FEE,
    BITCOIN_LATEST_BLOCK_HEIGHT, BITCOIN_LATEST_BLOCK_INPUTS, BITCOIN_LATEST_BLOCK_OUTPUTS,
    BITCOIN_LATEST_BLOCK_SIZE, BITCOIN_LATEST_BLOCK_TXS, BITCOIN_LATEST_BLOCK_VALUE,
    BITCOIN_LATEST_BLOCK_WEIGHT, BITCOIN_MEMPOOL_BYTES, BITCOIN_MEMPOOL_SIZE,
    BITCOIN_MEMPOOL_UNBROADCAST, BITCOIN_MEMPOOL_USAGE, BITCOIN_NUM_CHAINTIPS, BITCOIN_PEERS,
    BITCOIN_SIZE_ON_DISK, BITCOIN_TOTAL_BYTES_RECV, BITCOIN_TOTAL_BYTES_SENT, BITCOIN_UPTIME,
    BITCOIN_VERIFICATION_PROGRESS, BITCOIN_WARNINGS, SMART_FEE_2, SMART_FEE_20, SMART_FEE_3,
    SMART_FEE_5,
};

fn get_metrics(rpc: Arc<Client>) -> ClientResult<()> {
    // use scopes to visualize variables dependencies and divide by async tasks later
    {
        let networkinfo = rpc.get_network_info()?;
        {
            let blockchaininfo = rpc.get_blockchain_info()?;
            BITCOIN_BLOCKS.set(blockchaininfo.blocks as f64);
            BITCOIN_DIFFICULTY.set(blockchaininfo.difficulty as f64);
            BITCOIN_SIZE_ON_DISK.set(blockchaininfo.size_on_disk as f64);
            BITCOIN_VERIFICATION_PROGRESS.set(blockchaininfo.verification_progress as f64);

            {
                let uptime = rpc.uptime()?;
                BITCOIN_UPTIME
                    .with_label_values(&[
                        &networkinfo.version.to_string(),
                        &networkinfo.protocol_version.to_string(),
                        blockchaininfo.chain.to_core_arg(),
                    ])
                    .set(uptime as f64);
            }

            {
                let block_info = rpc.get_block_info(&blockchaininfo.best_block_hash)?;
                let latest_blockstats = rpc.get_block_stats(block_info.height as u64)?;

                BITCOIN_LATEST_BLOCK_SIZE.set(latest_blockstats.total_size as f64);
                BITCOIN_LATEST_BLOCK_TXS.set(latest_blockstats.txs as f64);
                BITCOIN_LATEST_BLOCK_HEIGHT.set(latest_blockstats.height as f64);
                BITCOIN_LATEST_BLOCK_WEIGHT.set(latest_blockstats.total_weight as f64);
                BITCOIN_LATEST_BLOCK_INPUTS.set(latest_blockstats.ins as f64);
                BITCOIN_LATEST_BLOCK_OUTPUTS.set(latest_blockstats.outs as f64);
                BITCOIN_LATEST_BLOCK_VALUE.set(latest_blockstats.total_out.to_btc() as f64);
                BITCOIN_LATEST_BLOCK_FEE.set(latest_blockstats.total_fee.to_btc() as f64);
            }
        }

        BITCOIN_PEERS.set(networkinfo.connections as f64);

        if let Some(connections_in) = networkinfo.connections_in {
            BITCOIN_CONN_IN.set(connections_in as f64);
        }
        if let Some(connections_out) = networkinfo.connections_out {
            BITCOIN_CONN_OUT.set(connections_out as f64);
        }

        match networkinfo.warnings {
            StringOrStringArray::String(value) if !value.is_empty() => BITCOIN_WARNINGS.inc(),
            StringOrStringArray::StringArray(values) => {
                BITCOIN_WARNINGS.inc_by(values.len() as f64);
            }
            _ => {}
        }
    }

    {
        let smartfee = rpc.estimate_smart_fee(2, None)?;
        if let Some(fee_rate) = smartfee.fee_rate {
            SMART_FEE_2.set(fee_rate.to_sat() as f64)
        }
    }

    {
        let smartfee = rpc.estimate_smart_fee(3, None)?;
        if let Some(fee_rate) = smartfee.fee_rate {
            SMART_FEE_3.set(fee_rate.to_sat() as f64)
        }
    }

    {
        let smartfee = rpc.estimate_smart_fee(5, None)?;
        if let Some(fee_rate) = smartfee.fee_rate {
            SMART_FEE_5.set(fee_rate.to_sat() as f64)
        }
    }

    {
        let smartfee = rpc.estimate_smart_fee(20, None)?;
        if let Some(fee_rate) = smartfee.fee_rate {
            SMART_FEE_20.set(fee_rate.to_sat() as f64)
        }
    }

    {
        let hashps = rpc.get_network_hash_ps(Some(120), None)?;
        BITCOIN_HASHPS.set(hashps);
    }

    {
        let hashps = rpc.get_network_hash_ps(Some(1), None)?;
        BITCOIN_HASHPS_1.set(hashps);
    }

    {
        let banned = rpc.list_banned()?;
        for ban in banned.iter() {
            BITCOIN_BAN_CREATED
                .with_label_values(&[&ban.address, "manually added"])
                .set(ban.ban_created as f64);
            BITCOIN_BANNED_UNTIL
                .with_label_values(&[&ban.address, "manually added"])
                .set(ban.banned_until as f64);
        }
    }

    {
        let chaintips = rpc.get_chain_tips()?;
        BITCOIN_NUM_CHAINTIPS.set(chaintips.len() as f64);
    }

    {
        let mempool = rpc.get_mempool_info()?;
        BITCOIN_MEMPOOL_BYTES.set(mempool.bytes as f64);
        BITCOIN_MEMPOOL_SIZE.set(mempool.size as f64);
        BITCOIN_MEMPOOL_USAGE.set(mempool.usage as f64);
        BITCOIN_MEMPOOL_UNBROADCAST.set(mempool.unbroadcast_count.unwrap_or_default() as f64);
    }

    {
        let netotals = rpc.get_net_totals()?;
        BITCOIN_TOTAL_BYTES_RECV.set(netotals.total_bytes_recv as f64);
        BITCOIN_TOTAL_BYTES_SENT.set(netotals.total_bytes_sent as f64);
    }

    Ok(())
}

/// Create Prometheus metrics to track bitcoind stats.
pub(crate) async fn serve_req(
    req: Request<Body>,
    addr: SocketAddr,
    rpc: Arc<Client>,
) -> ClientResult<Response<Body>> {
    if req.method() != Method::GET || req.uri().path() != "/metrics" {
        log::warn!("  [{}] {} {}", addr, req.method(), req.uri().path());
        return Ok(Response::builder()
            .status(404)
            .body(Body::default())
            .unwrap());
    }

    let response = match get_metrics(rpc) {
        Ok(_) => {
            let metric_families = prometheus::gather();
            let encoder = TextEncoder::new();
            let mut buffer = vec![];
            encoder.encode(&metric_families, &mut buffer).unwrap();

            Response::builder()
                .status(200)
                .header(CONTENT_TYPE, encoder.format_type())
                .body(Body::from(buffer))
                .unwrap()
        }
        Err(e) => Response::builder()
            .status(404)
            .header(CONTENT_TYPE, "text/plain")
            .body(Body::from(e.to_string()))
            .unwrap(),
    };
    Ok(response)
}
