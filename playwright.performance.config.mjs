import config from './playwright.config.mjs';
export default {...config, testMatch: '**/performance.spec.mjs', testIgnore: [], timeout: 60000};
