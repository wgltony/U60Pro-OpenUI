import { t } from '../../i18n'
import { PageHeader } from '../../ui/primitives'
import UplinkTab from './UplinkTab'

/** A direct entry to the same controls available under Network → Wi-Fi relay. */
export default function UplinkPage() {
  return (
    <div className="space-y-4">
      <PageHeader
        title={t('Wi-Fi repeater')}
        sub={t('Join an upstream Wi-Fi network (home, hotel, office) and use it instead of cellular data.')}
      />
      <UplinkTab />
    </div>
  )
}
