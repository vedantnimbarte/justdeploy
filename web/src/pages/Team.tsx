import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { api, ROLE_HINT, ROLE_LABEL, timeAgo, type ActivityEntry, type Invite, type Person, type Role } from "../api";
import { useMe } from "../me";
import { Badge, Band, Button, Card, CodeLine, ErrorText, Field, Input, PageHeader, SectionTitle, Select, useToast } from "../ui";

const ROLES: Role[] = ["viewer", "member", "admin", "owner"];

export default function TeamPage() {
  const { can } = useMe();
  return (
    <>
      <PageHeader title="Team" lead="Who can use this server, and what they did." />
      <Band className="grid max-w-4xl gap-6">
        <People />
        {can("owner") && <Invites />}
        {can("admin") && <Activity />}
      </Band>
    </>
  );
}

function People() {
  const toast = useToast();
  const queryClient = useQueryClient();
  const { me, can } = useMe();
  const people = useQuery({ queryKey: ["users"], queryFn: () => api.get<Person[]>("/users") });
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["users"] });
  const update = useMutation({
    mutationFn: ({ id, ...body }: { id: string; role?: Role; reset_two_factor?: boolean }) => api.patch(`/users/${id}`, body),
    onSuccess: (_, v) => {
      toast(v.reset_two_factor ? "Two-factor sign-in turned off for them" : "Role changed");
      refresh();
    },
    onError: (e) => toast(e.message, true),
  });
  const remove = useMutation({
    mutationFn: (id: string) => api.del(`/users/${id}`),
    onSuccess: () => {
      toast("Removed; their sessions and tokens stopped working");
      refresh();
    },
    onError: (e) => toast(e.message, true),
  });
  const owner = can("owner");

  return (
    <Card className="!p-0">
      <div className="px-5 pt-5 md:px-6 md:pt-6">
        <SectionTitle>People</SectionTitle>
      </div>
      <ul className="divide-y divide-line border-t border-line">
        {people.data?.map((p) => (
          <li key={p.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-3.5 md:px-6">
            <div className="min-w-0 flex-1">
              <p className="truncate font-medium">
                {p.email} {p.id === me?.id && <span className="text-sm font-normal text-muted">(you)</span>}
              </p>
              <p className="text-sm text-muted">
                Joined {timeAgo(p.created_at)}, two-factor {p.two_factor ? "on" : "off"}
              </p>
            </div>
            {owner && p.id !== me?.id ? (
              <>
                <label className="sr-only" htmlFor={`role-${p.id}`}>
                  Role of {p.email}
                </label>
                <Select
                  id={`role-${p.id}`}
                  className="!w-auto"
                  value={p.role}
                  onChange={(e) => update.mutate({ id: p.id, role: e.target.value as Role })}
                >
                  {ROLES.map((r) => (
                    <option key={r} value={r}>
                      {ROLE_LABEL[r]}
                    </option>
                  ))}
                </Select>
                {p.two_factor && (
                  <Button variant="tertiary" className="h-8" onClick={() => update.mutate({ id: p.id, reset_two_factor: true })}>
                    Turn off 2FA
                  </Button>
                )}
                <Button variant="tertiary" className="h-8" onClick={() => remove.mutate(p.id)}>
                  Remove
                </Button>
              </>
            ) : (
              <Badge>{ROLE_LABEL[p.role]}</Badge>
            )}
          </li>
        ))}
      </ul>
      <dl className="grid gap-x-6 gap-y-2 border-t border-line bg-subtle/60 px-5 py-4 text-sm md:grid-cols-[6rem_minmax(0,1fr)] md:px-6">
        {ROLES.map((r) => (
          <div key={r} className="contents">
            <dt className="font-medium">{ROLE_LABEL[r]}</dt>
            <dd className="text-muted">{ROLE_HINT[r]}</dd>
          </div>
        ))}
      </dl>
    </Card>
  );
}

function Invites() {
  const toast = useToast();
  const queryClient = useQueryClient();
  const invites = useQuery({ queryKey: ["invites"], queryFn: () => api.get<Invite[]>("/invites") });
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<Role>("member");
  const [link, setLink] = useState<{ email: string; url: string } | null>(null);
  const create = useMutation({
    mutationFn: () => api.post<{ invite: Invite; link: string }>("/invites", { email, role }),
    onSuccess: (r) => {
      setLink({ email: r.invite.email, url: r.link });
      setEmail("");
      queryClient.invalidateQueries({ queryKey: ["invites"] });
    },
  });
  const revoke = useMutation({
    mutationFn: (id: string) => api.del(`/invites/${id}`),
    onSuccess: () => {
      toast("Invite revoked; its link no longer works");
      queryClient.invalidateQueries({ queryKey: ["invites"] });
    },
  });

  return (
    <Card>
      <SectionTitle>Invite someone</SectionTitle>
      <p className="mb-5 max-w-[68ch] text-muted">
        You get a link to send them however you like. It works once, for that email, for 7 days.
      </p>
      <form onSubmit={(e: FormEvent) => (e.preventDefault(), create.mutate())} className="grid gap-3 md:grid-cols-[minmax(0,1fr)_10rem_auto] md:items-end">
        <Field label="Email">
          <Input type="email" value={email} onChange={(e) => setEmail(e.target.value)} placeholder="sam@example.com" required />
        </Field>
        <Field label="Role">
          <Select value={role} onChange={(e) => setRole(e.target.value as Role)}>
            {ROLES.filter((r) => r !== "owner").map((r) => (
              <option key={r} value={r}>
                {ROLE_LABEL[r]}
              </option>
            ))}
          </Select>
        </Field>
        <Button type="submit" busy={create.isPending}>
          Create invite link
        </Button>
      </form>
      <div className="mt-2">
        <ErrorText error={create.error} />
      </div>
      {link && (
        <div className="mt-5 rounded-panel border border-ok/40 bg-ok/5 p-4">
          <p className="mb-2 text-sm font-medium">Send this to {link.email}. It's shown only now.</p>
          <CodeLine value={link.url} />
        </div>
      )}
      {invites.data && invites.data.length > 0 && (
        <>
          <h3 className="mt-6 mb-2 text-sm font-semibold">Waiting to be accepted</h3>
          <ul className="divide-y divide-line rounded-panel border border-line">
            {invites.data.map((i) => (
              <li key={i.id} className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                <div className="min-w-0">
                  <p className="truncate font-medium">{i.email}</p>
                  <p className="text-sm text-muted">
                    {ROLE_LABEL[i.role]}, sent {timeAgo(i.created_at)}
                  </p>
                </div>
                <Button variant="tertiary" className="h-8" onClick={() => revoke.mutate(i.id)}>
                  Revoke
                </Button>
              </li>
            ))}
          </ul>
        </>
      )}
    </Card>
  );
}

function Activity() {
  const activity = useInfiniteQuery({
    queryKey: ["activity"],
    queryFn: ({ pageParam }) => api.get<ActivityEntry[]>(`/activity${pageParam ? `?before=${pageParam}` : ""}`),
    initialPageParam: 0,
    getNextPageParam: (last) => (last.length === 100 ? last.at(-1)?.id : undefined),
  });
  const entries = activity.data?.pages.flat() ?? [];

  return (
    <Card className="!p-0">
      <div className="px-5 pt-5 md:px-6 md:pt-6">
        <SectionTitle>Activity</SectionTitle>
      </div>
      {entries.length === 0 ? (
        <p className="px-5 pb-6 text-muted md:px-6">Nothing yet. Changes anyone makes show up here.</p>
      ) : (
        <ol className="divide-y divide-line border-t border-line">
          {entries.map((e) => (
            <li key={e.id} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 px-5 py-2.5 text-[14px] md:px-6">
              <p className="min-w-0">
                <span className="font-medium">{e.actor ?? "justdeploy"}</span> {e.action}
                {e.target && <> <span className="font-medium">{e.target}</span></>}
                {e.detail && <span className="text-muted">: {e.detail}</span>}
              </p>
              <time className="shrink-0 text-[13px] text-muted" dateTime={e.created_at} title={new Date(e.created_at).toLocaleString()}>
                {timeAgo(e.created_at)}
              </time>
            </li>
          ))}
        </ol>
      )}
      {activity.hasNextPage && (
        <div className="border-t border-line px-5 py-3 md:px-6">
          <Button variant="tertiary" busy={activity.isFetchingNextPage} onClick={() => activity.fetchNextPage()}>
            Show older
          </Button>
        </div>
      )}
    </Card>
  );
}
