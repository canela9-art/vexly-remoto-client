# canela/ — archivos propios del fork VexlyRemoto

- `custom-cliente.txt` — perfil firmado para equipos de clientes (solo recibe, aprobación con clic).
- `custom-tecnico.txt` — perfil firmado para técnicos (entrada y salida, cuenta, libreta).
- `rebrand-macos.sh` — convierte el .dmg de RustDesk en VexlyRemoto.app con la config adentro.

Los custom.txt están firmados con la llave de Vexly y apuntan a remoto.vexlyapp.com. No son
secretos (servidor, key pública del hbbs, URL de la API): cualquier instalación los lleva.
Se regeneran desde el repo privado (client/scripts/sign-brand-profiles.py).
