/** @type {import('tailwindcss').Config} */
module.exports = {
    content: [
        './index.html',
        './src/**/*.{vue,js,ts}',
    ],
    darkMode: 'class',
    theme: {
        extend: {
            colors: {
                'transparent': 'transparent',
                'current': 'currentColor',
                'light': '#f8f9fa',
                'dark': '#282c34',
                'light-secondary': '#e9ecef',
                'dark-secondary': '#21252b',
            },
            width: {
                md: '28rem',
            },
            translate: {
                md: '28rem',
            },
        },
    },
    plugins: [],
}
