use esp_idf_svc::sys::{
    ESP_ERR_NOT_FOUND, esp, esp_vfs_fat_mount_config_t, esp_vfs_fat_spiflash_mount_rw_wl,
    wl_handle_t,
};

/// Mounts the FAT `storage` partition at `/storage` for `std::fs`, formatting it on first use.
/// Only the 16 MB table has one, so `false` on the other boards.
pub fn mount() -> anyhow::Result<bool> {
    let config = esp_vfs_fat_mount_config_t {
        format_if_mount_failed: true,
        max_files: 4,
        allocation_unit_size: 0,
        disk_status_check_enable: false,
        use_one_fat: false,
    };
    let mut handle: wl_handle_t = 0;
    let result = unsafe {
        esp_vfs_fat_spiflash_mount_rw_wl(
            c"/storage".as_ptr(),
            c"storage".as_ptr(),
            &config,
            &mut handle,
        )
    };
    if result == ESP_ERR_NOT_FOUND {
        return Ok(false);
    }
    esp!(result)?;
    Ok(true)
}
