//! Điện thoại giả chạy tay (thử UI không cần điện thoại thật): `tests/fake_phone` khai `browse` + `edit` (W-7, W-8),
//! sẵn vài thư mục / tệp. In địa chỉ; Ferry nhập tay địa chỉ đó rồi ghép bằng PIN `123456`. Chạy tới khi bị dừng.
//!
//! `cargo run -p ferry-core --example fake_phone`

#[path = "../tests/fake_phone/mod.rs"]
mod fake_phone;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let phone = fake_phone::FakePhone::start().await;
    phone.set_features(&["ipv6", "dirs", "batch", "browse", "edit"]);
    for (name, dir, len) in [
        ("IMG_0001.jpg", "DCIM/Camera", 2_400_000),
        ("IMG_0002.jpg", "DCIM/Camera", 2_100_000),
        ("note.txt", "Download", 1_200),
        ("clip.mp4", "Download/Phim", 7_340_032),
        ("tap 1.mkv", "Movies/Phim bộ", 9_000_000),
        ("ringtone.mp3", "Music", 400_000),
    ] {
        phone.add_file(name, Some(dir), vec![0; len]);
    }
    println!("fake phone at {} (PIN {})", phone.v4(), fake_phone::PIN);
    std::future::pending::<()>().await;
}
