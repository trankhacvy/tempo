/** @type {import('next').NextConfig} */
const nextConfig = {
    reactStrictMode: true,
    // The wallet-adapter packages and the bundled client reference a few
    // node-ish globals; keep webpack from trying to polyfill fs/etc.
    webpack: (config) => {
        config.resolve.fallback = { ...config.resolve.fallback, fs: false, path: false, crypto: false };
        return config;
    },
};

export default nextConfig;
