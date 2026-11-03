; Branded welcome page with the sidebar artwork (electron-builder's assisted installer has none by default)
!macro customWelcomePage
  !define MUI_WELCOMEPAGE_TITLE "Добро пожаловать в PS5 Launcher"
  !define MUI_WELCOMEPAGE_TEXT "Лаунчер игр в стиле PlayStation 5 для Windows.$\r$\n$\r$\n• Управление геймпадом и экранная клавиатура$\r$\n• Живые фоны, новости и трофеи Steam$\r$\n• Друзья, чат и Центр управления по кнопке Xbox$\r$\n• Обновляется автоматически$\r$\n$\r$\nНажмите «Далее», чтобы продолжить."
  !insertmacro MUI_PAGE_WELCOME
!macroend
