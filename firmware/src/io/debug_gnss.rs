use crate::io::{gps::GNSSFix, ubx};

#[inline]
fn clamp_u16_to_u8(x: u16) -> u8 {
    if x > u8::MAX as u16 { u8::MAX } else { x as u8 }
}

#[inline]
fn agc_pct(agc_count: u16) -> u8 {
    // u-blox: 0..8191 == 0..100%
    let pct = (agc_count as u32 * 100 + 4095) / 8191;
    clamp_u16_to_u8(pct as u16)
}

#[derive(Default, Debug, Clone)]
struct CnoStats {
    n: u16,
    used: u16,
    avg: u8,
    min: u8,
    max: u8,
}

fn cno_stats_from_navsig(sig: &ubx::nav::NAVSIG) -> CnoStats {
    let mut s = CnoStats { min: 255, ..Default::default() };
    if sig.signals.is_empty() {
        s.min = 0;
        return s;
    }

    let mut sum: u32 = 0;
    for info in &sig.signals {
        let c = info.cno;
        s.n += 1;
        sum += c as u32;
        s.min = s.min.min(c);
        s.max = s.max.max(c);

        // Heuristic "used": any non-zero correction source.
        // If you later decode a real "used" bit from flags, swap this.
        if !matches!(info.corr_source, ubx::nav::CorrectionSource::NoCorrections) {
            s.used += 1;
        }
    }
    s.avg = (sum / (s.n.max(1) as u32)) as u8;
    s
}

#[derive(Default, Debug, Clone)]
struct CorrCounts {
    none: u16,
    sbas: u16,
    bds:  u16,
    rtcm2: u16,
    rtcm3_osr: u16,
    rtcm3_ssr: u16,
    slas: u16,
    spartn: u16,
    clas: u16,
    invalid: u16,
}

fn corr_counts_from_navsig(sig: &ubx::nav::NAVSIG) -> CorrCounts {
    use ubx::nav::CorrectionSource::*;
    let mut c = CorrCounts::default();
    for info in &sig.signals {
        match info.corr_source {
            NoCorrections        => c.none += 1,
            SBASCorrections      => c.sbas += 1,
            BeiDouCorrections    => c.bds += 1,
            RTCM2Corrections     => c.rtcm2 += 1,
            RTCM3OSRCorrections  => c.rtcm3_osr += 1,
            RTCM3SSRCorrections  => c.rtcm3_ssr += 1,
            QZSSSLASCorrections  => c.slas += 1,
            SPARTNCorrections    => c.spartn += 1,
            CLASCorrections      => c.clas += 1,
            Invalid              => c.invalid += 1,
        }
    }
    c
}

#[inline]
fn cno_quality(avg: u8) -> &'static str {
    if avg >= 40 { "good" }
    else if avg >= 32 { "ok" }
    else { "poor" }
}

#[inline]
fn corr_quality(cc: &CorrCounts) -> &'static str {
    // Priority order: highest-precision first
    if cc.spartn > 0 { "SP" }          // SPARTN
    else if cc.rtcm3_ssr > 0 { "SSR" } // RTCM 3 SSR
    else if cc.rtcm3_osr > 0 { "RTK" } // RTCM 3 OSR
    else if cc.rtcm2 > 0 { "DG" }      // RTCM2 / DGNSS
    else if (cc.sbas + cc.slas + cc.bds + cc.clas) > 0 { "AUG" } // augmentation
    else { "NO" }
}

#[inline]
fn jam_quality(cw: u8) -> &'static str {
    if cw >= 160 { "high" }
    else if cw >= 60 { "med" }
    else { "low" }
}

#[inline]
fn noise_quality(noise_per_ms: u16) -> &'static str {
    if noise_per_ms <= 300 { "low" }
    else if noise_per_ms <= 450 { "med" }
    else { "high" }
}

#[inline]
fn agc_quality(agc_percent: u8) -> &'static str {
    if agc_percent >= 70 { "high" }
    else if agc_percent >= 35 { "ok" }
    else { "low" }
}

/// Readable RF/signal debug. Designed to avoid wrapping.
pub fn log_gnss_rf_debug(fix: &GNSSFix) {
    // ---- NAV-SIG summary ----
    let s = cno_stats_from_navsig(&fix.nav_sig);
    tracing::info!(
        target: "RF",
        "SIG: used {}/{} | CNO avg {} min {} max {}",
        s.used, s.n, s.avg, s.min, s.max
    );

    let cc = corr_counts_from_navsig(&fix.nav_sig);
    tracing::info!(
        target: "RF",
        "CORR: none {} sbas {} bds {} r2 {} r3o {} r3s {} sp {}",
        cc.none, cc.sbas, cc.bds, cc.rtcm2, cc.rtcm3_osr, cc.rtcm3_ssr, cc.spartn
    );

    // ---- Subjective "how good is it" ----
    let mut rf_n: u16 = 0;
    let mut noise_sum: u32 = 0;
    let mut agc_sum: u32 = 0;
    let mut cw_max: u8 = 0;

    for b in &fix.mon_rf.rf_blocks {
        rf_n += 1;
        noise_sum += b.noise_per_ms as u32;
        agc_sum += agc_pct(b.agc_count) as u32;
        cw_max = cw_max.max(b.cw_suppression);
    }

    let noise_avg: u16 = if rf_n > 0 { (noise_sum / rf_n as u32) as u16 } else { 0 };
    let agc_avg: u8    = if rf_n > 0 { (agc_sum / rf_n as u32) as u8 } else { 0 };

    tracing::info!(
        target: "RF",
        "QUAL: sig {} | corr {} | jam {}",
        cno_quality(s.avg),
        corr_quality(&cc),
        jam_quality(cw_max),
    );

    tracing::info!(
        target: "RF",
        "QUAL2: noise {} | agc {}",
        noise_quality(noise_avg),
        agc_quality(agc_avg),
    );

    // ---- MON-RF per band (detailed) ----
    for (i, b) in fix.mon_rf.rf_blocks.iter().enumerate() {
        let agc = agc_pct(b.agc_count);
        tracing::info!(
            target: "RF",
            "RF{}: noise {} | agc {}% | cw_jam {}",
            i, b.noise_per_ms, agc, b.cw_suppression
        );
        tracing::info!(
            target: "RF",
            "RF{}: I {} Q {} | mag I {} Q {}",
            i, b.offset_i, b.offset_q, b.magnitude_i, b.magnitude_q
        );
    }
}
