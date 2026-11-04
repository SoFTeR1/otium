; Branded welcome page with the sidebar artwork (electron-builder's assisted installer has none by default)
!macro customWelcomePage
  !define MUI_WELCOMEPAGE_TITLE "Добро пожаловать в OTIUM"
  !define MUI_WELCOMEPAGE_TEXT "OTIUM — игровой лаунчер для Windows в стиле PS5. Создан для игры на телевизоре с геймпадом.$\r$\n$\r$\n• Управление геймпадом, экранная клавиатура, калибровка$\r$\n• Живые фоны, новости и трофеи Steam$\r$\n• Друзья, чат, голосовые комнаты и Центр управления$\r$\n• Обновляется автоматически$\r$\n$\r$\nНажмите «Далее», чтобы продолжить."
  !insertmacro MUI_PAGE_WELCOME
!macroend
