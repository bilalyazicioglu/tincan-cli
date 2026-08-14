//! tincan — terminalde çalışan, sunucusuz sesli sohbet.
//!
//! Mimarinin kalbi iki düzlemin ayrılması:
//!
//! * **Kontrol düzlemi** (yıldız): odayı ilk açan kişi koordinatördür; üye listesi,
//!   kanallar ve chat onun üzerinden akar. Küçük ve güvenilir bir trafiktir.
//! * **Ses düzlemi** (mesh): aynı kanaldaki peer'lar birbirine doğrudan bağlanır ve
//!   Opus paketlerini unreliable datagram olarak yollar. Koordinatörden geçmez.

pub mod auth;
pub mod invite;
pub mod proto;
pub mod room;

pub mod net;
pub mod ui;
pub mod audio;
