//! Automatic JVM and memory tuning recommendation based on available hardware and mod count.

/// Hardware-specific memory and garbage collector tuning recommendation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareRecommendation {
    pub memory_min_mb: u32,
    pub memory_max_mb: u32,
    pub jvm_args: Vec<String>,
    pub summary_ru: String,
}

/// Generates optimal JVM heap size and garbage collector flags.
pub fn recommend_jvm_tuning(
    total_ram_mb: u64,
    java_major: u32,
    mod_count: usize,
) -> HardwareRecommendation {
    let (memory_min_mb, memory_max_mb) = if total_ram_mb <= 4096 {
        let max_mb = 2048.min(total_ram_mb.saturating_sub(1024) as u32).max(1024);
        (1024, max_mb)
    } else if total_ram_mb <= 8192 {
        let max_mb = if mod_count > 100 {
            5120
        } else if mod_count > 30 {
            4096
        } else {
            3072
        };
        (2048, max_mb)
    } else if total_ram_mb <= 16384 {
        let max_mb = if mod_count > 150 {
            8192
        } else if mod_count > 50 {
            6144
        } else {
            4096
        };
        (3072, max_mb)
    } else {
        let max_mb = if mod_count > 200 {
            10240
        } else if mod_count > 50 {
            8192
        } else {
            6144
        };
        (4096, max_mb)
    };

    if java_major >= 21 && memory_max_mb >= 6144 {
        let jvm_args = vec![
            "-XX:+UnlockExperimentalVMOptions".to_string(),
            "-XX:+UseZGC".to_string(),
            "-XX:+ZGenerational".to_string(),
        ];
        let summary_ru = format!(
            "Generational ZGC (Java {java_major}) - RAM: {memory_min_mb}-{memory_max_mb} MB (ПК: {total_ram_mb} MB, модов: {mod_count})"
        );
        HardwareRecommendation {
            memory_min_mb,
            memory_max_mb,
            jvm_args,
            summary_ru,
        }
    } else {
        let jvm_args = vec![
            "-XX:+UnlockExperimentalVMOptions".to_string(),
            "-XX:+UseG1GC".to_string(),
            "-XX:G1NewSizePercent=20".to_string(),
            "-XX:G1ReservePercent=20".to_string(),
            "-XX:MaxGCPauseMillis=50".to_string(),
            "-XX:G1HeapRegionSize=32M".to_string(),
        ];
        let summary_ru = format!(
            "Tuned G1GC (Java {java_major}) - RAM: {memory_min_mb}-{memory_max_mb} MB (ПК: {total_ram_mb} MB, модов: {mod_count})"
        );
        HardwareRecommendation {
            memory_min_mb,
            memory_max_mb,
            jvm_args,
            summary_ru,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommend_jvm_tuning_zgc_on_java_21() {
        let rec = recommend_jvm_tuning(16384, 21, 80);
        assert_eq!(rec.memory_min_mb, 3072);
        assert_eq!(rec.memory_max_mb, 6144);
        assert!(rec.jvm_args.contains(&"-XX:+UseZGC".to_string()));
        assert!(rec.jvm_args.contains(&"-XX:+ZGenerational".to_string()));
    }

    #[test]
    fn test_recommend_jvm_tuning_g1gc_on_java_17() {
        let rec = recommend_jvm_tuning(16384, 17, 80);
        assert_eq!(rec.memory_min_mb, 3072);
        assert_eq!(rec.memory_max_mb, 6144);
        assert!(rec.jvm_args.contains(&"-XX:+UseG1GC".to_string()));
        assert!(!rec.jvm_args.contains(&"-XX:+UseZGC".to_string()));
    }
}
