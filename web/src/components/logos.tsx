/** Compact brand marks for the layer diagram. Paths adapted from Simple Icons (CC0). */

type LogoProps = {
  className?: string;
  /** Empty string marks the SVG decorative (label lives beside it). */
  title?: string;
};

function BrandMark({
  className,
  title = "",
  path,
}: LogoProps & { path: string }) {
  const decorative = title === "";
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      role={decorative ? "presentation" : "img"}
      aria-hidden={decorative || undefined}
      aria-label={decorative ? undefined : title}
    >
      {decorative ? null : <title>{title}</title>}
      <path fill="currentColor" d={path} />
    </svg>
  );
}

export function LogoClerk(props: LogoProps) {
  return (
    <BrandMark
      {...props}
      title={props.title ?? "Clerk"}
      path="M12 1 3 5.25v6.15c0 5.1 3.45 9.87 8.28 11.4a2.2 2.2 0 0 0 1.44 0C17.55 21.27 21 16.5 21 11.4V5.25L12 1Zm0 3.2 6.3 3v3.95c0 3.75-2.4 7.3-6.3 8.55-3.9-1.25-6.3-4.8-6.3-8.55V7.2L12 4.2Z"
    />
  );
}

export function LogoNeon(props: LogoProps) {
  return (
    <BrandMark
      {...props}
      title={props.title ?? "Neon"}
      path="M0 4.7v14.6h6.95V12.9L15.5 19.3H24v-4.5l-7.7-5.7H24V4.7H0Z"
    />
  );
}

export function LogoSupabase(props: LogoProps) {
  return (
    <BrandMark
      {...props}
      title={props.title ?? "Supabase"}
      path="M13.96 1.5a1.2 1.2 0 0 0-2.08.1L3.2 17.4a1.2 1.2 0 0 0 1.04 1.8h6.4v3.3a1.2 1.2 0 0 0 2.08-.1L20.8 6.6a1.2 1.2 0 0 0-1.04-1.8h-6.4V1.5Z"
    />
  );
}

export function LogoVercel(props: LogoProps) {
  return (
    <BrandMark
      {...props}
      title={props.title ?? "Vercel"}
      path="M12 2.4 24 21.6H0L12 2.4Z"
    />
  );
}

export function LogoRender(props: LogoProps) {
  return (
    <BrandMark
      {...props}
      title={props.title ?? "Render"}
      path="M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20Zm0 3.2a6.8 6.8 0 0 1 6.8 6.8H12V5.2Z"
    />
  );
}

export function LogoFly(props: LogoProps) {
  return (
    <BrandMark
      {...props}
      title={props.title ?? "Fly.io"}
      path="M12 1.5 2.5 7v10L12 22.5 21.5 17V7L12 1.5Zm0 3.1 6.4 3.7v7.4L12 19.4l-6.4-3.7V8.3L12 4.6Z"
    />
  );
}

export function LogoStripe(props: LogoProps) {
  return (
    <BrandMark
      {...props}
      title={props.title ?? "Stripe Projects"}
      path="M13.976 9.15c-2.172-.806-3.356-1.426-3.356-2.409 0-.831.683-1.305 1.901-1.305 2.227 0 4.515.858 6.09 1.631l.89-5.494C18.252.975 15.697 0 12.165 0 9.667 0 7.589.654 6.104 1.872 4.56 3.147 3.757 4.992 3.757 7.218c0 4.039 2.467 5.76 6.476 7.219 2.585.92 3.445 1.574 3.445 2.583 0 .98-.84 1.545-2.354 1.545-1.875 0-4.965-.921-6.99-2.109l-.9 5.555C5.175 22.99 8.385 24 11.714 24c2.641 0 4.843-.624 6.328-1.813 1.664-1.305 2.525-3.236 2.525-5.732 0-4.128-2.524-5.851-6.591-7.305h.000Z"
    />
  );
}
