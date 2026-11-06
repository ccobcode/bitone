/** 品牌标志,不随主题变化. */
export function BrandMark({ size }: { size: number }) {
    return (
        <svg width={size} height={size} viewBox="0 0 160 160" role="img" aria-label="BitOne" className="block shrink-0">
            <rect width="160" height="160" rx="36" fill="#0B0D11" />
            <path d="M44 52V108M116 52V108" stroke="#EEF1F5" strokeWidth="16" strokeLinecap="round" />
            <rect x="62" y="62" width="36" height="36" rx="9" fill="#1F5EFF" />
        </svg>
    );
}
