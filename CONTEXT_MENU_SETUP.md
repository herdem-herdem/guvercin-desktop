# Context Menu Integration for Guvercin

Bu özellik, kullanıcıların herhangi bir dosyaya sağ tıklayarak "Guvercin ile Gönder" seçeneğini görmelerini sağlar. Bu sayede dosyayı doğrudan mail olarak göndermek için compose penceresi açılır.

## Otomatik Kurulum ✨

**Windows & Linux**: Otomatik! Guvercin ilk kez çalıştığında, context menu kayıt edilir:
- **Windows**: Registry'ye otomatik eklenir
- **Linux**: Nautilus ve KDE menülerine otomatik eklenir

**macOS**: Context menu yerine Drag & Drop veya "Open With" kullan:
- Dosyayı Guvercin window'una sürükleyin, VEYA
- Sağ tıkla → "Open With..." → Guvercin seç

Uygulama sadece açın, kapayın, başını kaldırmayın - hepsi oldu! 🎉

## Dosya Ekleme Yolları

### Windows & Linux
1. **Context Menu**: Sağ tıkla → "Guvercin ile Gönder" (otomatik)

### macOS
1. **Drag & Drop**: Dosyayı Guvercin'e sürükle
2. **Open With**: Sağ tıkla → "Open With..." → Guvercin
3. **URI Scheme**: Terminal'den `open guvercin://attach-file?path=/path/to/file`

## Teknik Detaylar

### URI Scheme
- **Scheme**: `guvercin://attach-file?path=<dosya_yolu>`
- **Örnek**: `guvercin://attach-file?path=/home/user/document.pdf`

### Platform-Specific Implementation

#### Windows
- Registry entry otomatik olarak eklenir: `HKEY_CLASSES_ROOT\*\shell\GuvercinSend`
- Command: `guvercin.exe --file-attachment "<path>"`

#### Linux
- Nautilus script otomatik olarak oluşturulur: `~/.local/share/nautilus/scripts/Send with Guvercin`
- KDE service menu otomatik olarak oluşturulur: `~/.local/share/kio/servicemenus/guvercin-attach.desktop`
- URI: `xdg-open "guvercin://attach-file?path=<encoded_path>"`

#### macOS
- Context menu registrationı yapılmaz (macOS Services API karmaşık)
- Bunun yerine: Drag & Drop veya "Open With..." kullan

### Rust Backend
- **Command**: `attach_file_to_compose(file_path: String)`
  - Dosyayı okur ve base64 encode eder
  - Ek dosyayla compose penceresi açar
- **URI Handler**: Deep-link plugin tarafından Frontend'e gönderilir
- **Auto-Registration**: İlk başlangıçta `ContextMenuStore` tarafından otomatik kayıt edilir

### Frontend
- **Module**: `frontend/src/utils/attachmentInbox.js`
  - Deep-link event listener
  - URI parsing ve file attachment handling
  - Compose window açan Tauri command'ı çağırır

## Manuel Kaldırma (Uninstall)

Eğer context menu'yü kaldırmak isterseniz:

### Windows
```powershell
# Registry entry'yi sil
reg delete "HKEY_CLASSES_ROOT\*\shell\GuvercinSend" /f
```

### Linux
```bash
# Scripts ve service menu'ü kaldır
rm ~/.local/share/nautilus/scripts/"Send with Guvercin"
rm ~/.local/share/kio/servicemenus/guvercin-attach.desktop
```

## Sorun Giderme

### Linux
- Konteks menüsü görünmüyor
  - File manager'ı yeniden başlatın
  - Guvercin'i kapatıp yeniden açın (otomatik registration tetiklenecek)

### Tüm Platformlar
- Context menu hala kayıtlı değilse:
  - Guvercin'in uygulama verilerine yazma izni olduğundan emin olun
  - `~/.guvercin` veya `~/.config/guvercin` klasörü silin
  - Uygulamayı yeniden başlatın

## Güvenlik Notları

- ✅ Yalnızca Guvercin URI scheme'i kabul edilir
- ✅ Dosya yolu URL-encoded olur (special karakterler güvenli)
- ✅ Guvercin process'i sadece gerçek dosyaları okur
- ✅ Rastgele dosya okuma engellenir

## Supported File Types

Tüm dosya türleri desteklenir:
- Documents: PDF, Word (.docx), Excel (.xlsx), vb.
- Archives: ZIP, RAR, 7Z, vb.
- Media: MP3, MP4, JPG, PNG, vb.
- Code: .txt, .js, .py, vb.
- Email: .eml, .msg
- Ve daha pek çok format...

**Maksimum dosya boyutu**: İşletim sistemi ve mail sunucunuzun limitine bağlı (genellikle 25MB)

---

## Geliştiriciler İçin

`scripts/` klasöründe platform-specific kurulum scriptleri vardır. Bunlar:
- `register-context-menu-windows.ps1` - Windows manual setup
- `register-context-menu-macos.sh` - macOS manual setup
- `register-context-menu-linux.sh` - Linux manual setup

Normal kullanıcılar bunları çalıştırmasına gerek **yoktur** - otomatik olarak yapılır.
Sadece debugging veya manuel işlem gerekiyorsa kullanın.
