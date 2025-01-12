import { getServerSession } from 'next-auth';

import { authOptions } from '@/lib/auth';
import { fetcher } from '@/lib/utils';

export async function GET(
  req: Request,
  props: { params: Promise<{ projectId: string; pipelineVersionId: string }> }
): Promise<Response> {
  const params = await props.params;
  const projectId = params.projectId;
  const pipelineVersionId = params.pipelineVersionId;

  const session = await getServerSession(authOptions);
  const user = session!.user;

  return fetcher(
    `/projects/${projectId}/traces/workshop/${pipelineVersionId}`,
    {
      method: 'GET',
      headers: {
        Authorization: `Bearer ${user.apiKey}`
      }
    }
  );
}
