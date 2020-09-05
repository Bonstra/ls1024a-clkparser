use std::convert::TryFrom;
use std::io::prelude::*;

fn pll_simple_rate(buf: &[u8], inrate: u32, has_outdiv: bool) -> u32 {
    if buf.len() != 0x20 {
        panic!("buf must be 0x20 bytes wide for simple PLLs");
    }
    let m = u64::from(buf[0]) | u64::from(buf[4] & 0x3) << 8;
    let p = u64::from(buf[8] & 0x3f);
    let s = u32::from(buf[0xc] & 0x7);
    let ctl = buf[0x10];
    let reset = (ctl & 1) != 0;
    let bypass = (ctl & 0x10) != 0;

    if bypass {
        return inrate;
    }
    if reset && !bypass {
        return 0;
    }
    let rate = u32::try_from((u64::from(inrate) * m) / (p * 2u64.pow(s))).unwrap();
    if has_outdiv {
        let outdiv = u32::from(buf[0x1c] & 0x1f);
        let bypass = (buf[0x1c] & 0x80) != 0;
        if bypass {
            rate
        } else {
            rate / outdiv
        }
    } else {
        rate
    }
}

fn pll_dither_rate(buf: &[u8], inrate: u32) -> u32 {
    if buf.len() != 0x30 {
        panic!("buf must be 0x30 bytes wide for dithering PLLs");
    }
    let m = u64::from(buf[0]) | u64::from(buf[4] & 0x1) << 8;
    let p = u64::from(buf[8] & 0x3f);
    let s = u32::from(buf[0xc] & 0x7);
    let k = u64::from(buf[0x20]) | u64::from(buf[0x24] & 0xf) << 8;
    let ctl = buf[0x10];
    let reset = (ctl & 1) != 0;
    let bypass = (ctl & 0x10) != 0;

    if bypass {
        return inrate;
    }
    if reset && !bypass {
        return 0;
    }
    let num = u64::from(inrate) * (m * 1024 + k);
    let denom = p * 2u64.pow(s);
    // The "+ 512" part rounds to the nearest Hz
    u32::try_from((num / denom + 511) / 1024).unwrap()
}

fn clkgen_rate(ctl: u8, divctl: Option<u8>, srcs: &[u32], bypass: bool) -> (bool, u32) {
    let on = (ctl & 1) == 1;
    let mux = ((ctl >> 1) & 7) as usize;
    let inrate = if mux >= srcs.len() {
        eprintln!("Warning: mux {} is outside known range.", mux);
        0
    } else {
        srcs[mux]
    };

    if let Some(divctl) = divctl {
        let div = u32::from(divctl & 0x1f);
        if bypass {
            return (on, inrate);
        }
        if div < 2 {
            eprintln!("Warning: divider value is less than 2.");
            return (on, 0);
        }
        (on, inrate / div)
    } else {
        (on, inrate)
    }
}

fn axigate_is_on(ctl: u8, bit: u8) -> bool {
    return ctl & (1 << bit) != 0;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() != 3 {
        eprintln!("Invalid number of arguments.");
        eprintln!("Usage: {} CLKDUMPFILE XTALRATE", &args[0]);
        eprintln!("Where CLKDUMPFILE is a file containing a binary dump of registers values");
        eprintln!("from the CLKRESET region (0x904b0000-0x904b03ff).");
        eprintln!("And where the crystal rate, XTALRATE, is 24 or 48 (MHz)");
        std::process::exit(1);
    }

    let mut file = std::fs::File::open(&args[1]).expect("Failed to open file.");
    let mut xtal: u32 = (&args[2]).parse().expect("Invalid number");
    if xtal != 24 && xtal != 48 {
        eprintln!("XTALRATE must be either 24 or 48.");
        std::process::exit(1);
    }
    xtal *= 1000000;

    let mut buf = [0u8; 0x400];
    file.read_exact(&mut buf)
        .expect("Failed to read first 0x400 bytes");

    let mut genplls = [0u32; 4];
    for i in 0..3 {
        let o = 0x1c0 + i * 0x20;
        let has_outdiv = i == 0 || i == 1;
        let rate = pll_simple_rate(&buf[o..(o + 0x20)], xtal, has_outdiv);
        genplls[i] = rate;
    }
    genplls[3] = pll_dither_rate(&buf[0x220..0x250], xtal);

    let mut outplls = [0u32; 4];
    let pllmask = buf[0x38];
    for i in 0..4 {
        outplls[i] = if (pllmask & (1 << i)) != 0 {
            xtal
        } else {
            genplls[i]
        };
    }

    if (buf[0x34] & 0x1) != 0 {
        eprintln!("PLL global bypass bit is set!");
        eprintln!("I don't know what effects it has on the clock generators. I can't continue.");
        std::process::exit(1);
    }

    let pllmux = [outplls[0], outplls[1], outplls[2], outplls[3], xtal];
    let mut clkgens = [
        ("axi", false, buf[0x40], Some(buf[0x4c]), false, 0u32),
        ("a9dp", false, buf[0x80], Some(buf[0x84]), false, 0u32),
        ("l2cc", false, buf[0x90], Some(buf[0x94]), false, 0u32),
        ("tpi", false, buf[0xa0], Some(buf[0xa4]), false, 0u32),
        ("csys", false, buf[0xb0], Some(buf[0xb4]), false, 0u32),
        ("extphy0", false, buf[0xc0], Some(buf[0xc4]), false, 0u32),
        ("extphy1", false, buf[0xd0], Some(buf[0xd4]), false, 0u32),
        ("extphy2", false, buf[0xe0], Some(buf[0xe4]), false, 0u32),
        ("ddr", false, buf[0xf0], Some(buf[0xf4]), false, 0u32),
        ("pfe", false, buf[0x100], Some(buf[0x104]), false, 0u32),
        ("ipsec", false, buf[0x110], Some(buf[0x114]), false, 0u32),
        ("dect", false, buf[0x120], Some(buf[0x124]), false, 0u32),
        ("gemtx", false, buf[0x130], Some(buf[0x134]), false, 0u32),
        ("tdmntg", false, buf[0x140], Some(buf[0x144]), false, 0u32),
        ("tsuntg", false, buf[0x150], Some(buf[0x154]), false, 0u32),
        ("sata_pmu", false, buf[0x160], Some(buf[0x164]), false, 0u32),
        ("sata_oob", false, buf[0x170], Some(buf[0x174]), false, 0u32),
        ("sata_occ", false, buf[0x180], Some(buf[0x184]), false, 0u32),
        ("pcie_occ", false, buf[0x190], Some(buf[0x194]), false, 0u32),
        ("sgmii_occ", false, buf[0x1a0], Some(buf[0x1a4]), false, 0u32),
    ];
    for i in 0..clkgens.len() {
        let bypass = clkgens[i].1;
        let ctl = clkgens[i].2;
        let divctl = clkgens[i].3;
        let (on, rate) = clkgen_rate(ctl, divctl, &pllmux, bypass);
        clkgens[i].4 = on;
        clkgens[i].5 = rate;
    }

    let mut axigates = [
        ("0_4", buf[0x40], 4, false),
        ("dpi_cie", buf[0x40], 5, false),
        ("dpi_decomp", buf[0x40], 6, false),
        ("0_7", buf[0x40], 7, false),
        ("dus", buf[0x44], 0, false),
        ("ipsec_eape", buf[0x44], 1, false),
        ("ipsec_spacc", buf[0x44], 2, false),
        ("pfe_sys", buf[0x44], 3, false),
        ("tdm", buf[0x44], 4, false),
        ("i2cspi", buf[0x44], 5, false),
        ("uart", buf[0x44], 6, false),
        ("rtc", buf[0x44], 7, false),
        ("pcie0", buf[0x48], 0, false),
        ("pcie1", buf[0x48], 1, false),
        ("sata", buf[0x48], 2, false),
        ("usb0", buf[0x48], 3, false),
        ("usb1", buf[0x48], 4, false),
        ("2_5", buf[0x48], 5, false),
        ("2_6", buf[0x48], 6, false),
        ("2_7", buf[0x48], 7, false),
    ];
    for i in 0..axigates.len() {
        let ctl = axigates[i].1;
        let bit = axigates[i].2;
        axigates[i].3 = axigate_is_on(ctl, bit);
    }

    for i in 0..4 {
        println!(
            "PLL{} - Generated: {} Hz - Output: {} Hz",
            i, genplls[i], outplls[i]
        );
    }
    for gen in &clkgens {
        println!("clkgen \"{}\": {} Hz ({})", gen.0, gen.5, if gen.4 { "ON" } else { "OFF" });
    }
    for gate in &axigates {
        println!("axigate \"{}\": {}", gate.0, if gate.3 { "ON" } else { "OFF" })
    }
}
