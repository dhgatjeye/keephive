pub const WINDOWS_RESERVED_NAMES: &[&str; 24] = &[
    "aux", "con", "nul", "prn",
    "com0", "com1", "com2", "com3", "com4",
    "com5", "com6", "com7", "com8", "com9",
    "lpt0", "lpt1", "lpt2", "lpt3", "lpt4",
    "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

pub fn is_reserved_name(name: &str) -> bool {
    let lowercase = name.to_lowercase();
    let base_name = lowercase.split('.').next().unwrap_or(&lowercase);
    WINDOWS_RESERVED_NAMES.contains(&base_name)
}